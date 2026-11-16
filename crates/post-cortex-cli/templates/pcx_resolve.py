"""post-cortex project and backend resolution for the Claude Code hooks.

Imported by every pcx-*.py hook (they put their own directory on sys.path,
since `python3 -I` leaves it out). Standard library only, and nothing here
raises: a hook that cannot resolve prints nothing and the session carries on.
A backend that refuses the resolution (a 4xx answer, say a key limited to
other workspaces) gets its message written to stderr, so the reason shows.

The rules are the hybrid contract's, shared with `pcx` (pcx/project.rs and
pcx/profiles.rs) and the native plugins; the test vectors live in
integrations/testdata/project-keys.json.

Backend (resolve_server):
    url    PC_URL, else CLAUDE_PLUGIN_OPTION_URL (the plugin's `url` option),
           else the profile's url in config.json (profile = PC_PROFILE, else
           `active`, else "cloud").
    token  PC_AUTH_TOKEN, else CLAUDE_PLUGIN_OPTION_API_KEY, else the
           profile's token in credentials.json (for the profile whose url
           is the one in use).
Both files live in $PC_CONFIG_DIR, else ~/.post-cortex.

Project (project_ids), first hit wins:
    1. .post-cortex.json {"session_id","workspace_id"} at the git root
       (written by `pcx setup --project`);
    2. the ids in CLAUDE.md at the git root (projects set up before);
    3. the cache ~/.post-cortex/projects.json, keyed by project key, valid
       for the same backend url and the same identity: the first 16 hex of
       the sha256 of the bearer token (never the token), absent without
       one. An entry written before identities (no "identity") is a miss
       for a backend reached with a token, so it is resolved again and
       overwritten; ids the backend refuses (HTTP 403 or 404) are dropped;
    4. POST /api/projects/resolve {key, name, client, workspace}, then cached.

Project key: the git `origin` URL normalised (no scheme, user, port or
`.git`; `git@host:a/b` is `host/a/b`; host lowercased), else
`local:<sha256 of the git root path, first 16 hex>`, else the same for the
working directory.
"""

import hashlib
import json
import os
import re
import subprocess
import sys
import urllib.error
import urllib.request

CLOUD_URL = "https://api.pcx.is"
SELF_URL = "http://127.0.0.1:3737"
AUTHOR = "claude-code"
PINNED_FILE = ".post-cortex.json"
MAX_ERROR_CHARS = 500
UUID = r"[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}"
SESSION_RE = re.compile(r"Session ID\W*(" + UUID + ")")
WORKSPACE_RE = re.compile(r"Workspace ID\W*(" + UUID + ")")
SCHEME_RE = re.compile(r"^([A-Za-z][A-Za-z0-9+.-]*)://(.*)$", re.S)


def _clean(value):
    return value.strip() if isinstance(value, str) and value.strip() else None


def config_dir():
    return _clean(os.environ.get("PC_CONFIG_DIR")) or os.path.join(
        os.path.expanduser("~"), ".post-cortex"
    )


def _read_json(path):
    try:
        with open(path, encoding="utf-8") as f:
            value = json.load(f)
    except (OSError, ValueError):
        return {}
    return value if isinstance(value, dict) else {}


def _write_json(path, value):
    """Write `value` to `path` through a temp file; errors are ignored."""
    try:
        os.makedirs(os.path.dirname(path), exist_ok=True)
        tmp = "%s.%d.tmp" % (path, os.getpid())
        with open(tmp, "w", encoding="utf-8") as f:
            json.dump(value, f, indent=2)
            f.write("\n")
        os.replace(tmp, path)
    except OSError:
        pass


# ------------------------------------------------------------- backend


def _config():
    return _read_json(os.path.join(config_dir(), "config.json"))


def _profile_url(config, name):
    entry = (config.get("profiles") or {}).get(name)
    if isinstance(entry, dict) and _clean(entry.get("url")):
        return entry["url"].strip().rstrip("/")
    return {"cloud": CLOUD_URL, "self": SELF_URL}.get(name)


def _stored_token(profile):
    creds = _read_json(os.path.join(config_dir(), "credentials.json"))
    entry = creds.get(profile) if profile else None
    return _clean(entry.get("token")) if isinstance(entry, dict) else None


def resolve_server(url=None, profile=None):
    """(url, token, profile) of the backend; `url` and `profile` are what a
    project hook command passes (they win). url is None when unknown."""
    config = _config()
    # A project hook that passes only a url talks to a self-hosted daemon.
    name = (
        _clean(profile)
        or ("self" if _clean(url) else None)
        or _clean(os.environ.get("PC_PROFILE"))
        or _clean(config.get("active"))
        or "cloud"
    )
    explicit = _clean(url) or _clean(os.environ.get("PC_URL"))
    url = (
        explicit
        or _clean(os.environ.get("CLAUDE_PLUGIN_OPTION_URL"))
        or _profile_url(config, name)
    )
    url = url.rstrip("/") if url else None
    token = _clean(os.environ.get("PC_AUTH_TOKEN")) or _clean(
        os.environ.get("CLAUDE_PLUGIN_OPTION_API_KEY")
    )
    if not token and url:
        if explicit or _profile_url(config, name) == url:
            token = _stored_token(name)
        else:
            # The plugin's url: the key of a profile that points there.
            others = set(config.get("profiles") or {}) | {"cloud", "self"}
            for candidate in sorted(others):
                if _profile_url(config, candidate) == url:
                    token = _stored_token(candidate)
                    if token:
                        break
    return url, token, name


def host_settings(author=AUTHOR):
    """hosts.<author> of config.json: enabled (default True), capture
    (default False), workspace (default None)."""
    host = (_config().get("hosts") or {}).get(author)
    host = host if isinstance(host, dict) else {}
    return {
        "enabled": host.get("enabled") is not False,
        "capture": host.get("capture") is True,
        "workspace": _clean(host.get("workspace")),
    }


# ------------------------------------------------------------- project


def normalize_remote(url):
    """`host/path` of a git remote URL, or None when it names no host."""
    url = (url or "").strip()
    if not url or "\\" in url:
        return None
    match = SCHEME_RE.match(url)
    if match:
        if match.group(1).lower() == "file":
            return None
        authority, _, path = match.group(2).partition("/")
        host = authority.rsplit("@", 1)[-1]
        host = host.split("]", 1)[0] + "]" if host.startswith("[") else host.split(":", 1)[0]
    else:
        colon, slash = url.find(":"), url.find("/")
        if colon <= 0 or (slash != -1 and slash < colon):
            return None
        host = url[:colon].rsplit("@", 1)[-1]
        path = url[colon + 1 :]
        if len(host) < 2:  # a Windows drive letter
            return None
    host = host.lower()
    path = path.strip("/")
    if path.endswith(".git"):
        path = path[:-4].rstrip("/")
    if not host or not path:
        return None
    return host + "/" + path


def _sha256_16(text):
    return hashlib.sha256(text.encode("utf-8")).hexdigest()[:16]


def local_key(root):
    return "local:" + _sha256_16(root)


def identity(token):
    """Who a cached entry was resolved as: the first 16 hex of the sha256 of
    the bearer token (never the token itself), None without one."""
    token = _clean(token)
    return _sha256_16(token) if token else None


def git_root(directory):
    """The nearest directory at or above `directory` holding `.git`."""
    current = directory
    while True:
        if os.path.exists(os.path.join(current, ".git")):
            return current
        parent = os.path.dirname(current)
        if parent == current:
            return None
        current = parent


def git_origin(root):
    try:
        out = subprocess.run(
            ["git", "-C", root, "config", "--get", "remote.origin.url"],
            capture_output=True,
            text=True,
            timeout=3,
        )
    except (OSError, subprocess.SubprocessError):
        return None
    return _clean(out.stdout) if out.returncode == 0 else None


def project(directory):
    """(key, name, root) of the project at `directory`."""
    directory = os.path.realpath(directory)
    root = git_root(directory)
    if root is not None:
        remote = normalize_remote(git_origin(root))
        if remote:
            return remote, remote.rsplit("/", 1)[-1], root
    root = root or directory
    return local_key(root), os.path.basename(root) or root, root


def pinned_ids(root, files=("CLAUDE.md",)):
    """{"session_id","workspace_id"} fixed in the repository, or None."""
    pinned = _read_json(os.path.join(root, PINNED_FILE))
    session, workspace = _clean(pinned.get("session_id")), _clean(pinned.get("workspace_id"))
    if session and workspace:
        return {"session_id": session, "workspace_id": workspace}
    for name in files:
        try:
            with open(os.path.join(root, name), encoding="utf-8") as f:
                text = f.read()
        except (OSError, ValueError):
            continue
        session, workspace = SESSION_RE.search(text), WORKSPACE_RE.search(text)
        if session and workspace:
            return {"session_id": session.group(1), "workspace_id": workspace.group(1)}
    return None


def _cache_path():
    return os.path.join(config_dir(), "projects.json")


def _entry_ids(entry, url, token):
    """The ids of a cache entry when it is for the backend at `url` and the
    identity of `token`, else None."""
    if not isinstance(entry, dict) or (entry.get("url") or "").rstrip("/") != url.rstrip("/"):
        return None
    if _clean(entry.get("identity")) != identity(token):
        return None
    if _clean(entry.get("session_id")) and _clean(entry.get("workspace_id")):
        return {"session_id": entry["session_id"], "workspace_id": entry["workspace_id"]}
    return None


def cached_ids(key, url, token):
    return _entry_ids(_read_json(_cache_path()).get(key), url, token)


def cache_ids(key, url, token, ids):
    cache = _read_json(_cache_path())
    entry = {
        "workspace_id": ids["workspace_id"],
        "session_id": ids["session_id"],
        "url": url.rstrip("/"),
    }
    if identity(token):
        entry["identity"] = identity(token)
    cache[key] = entry
    _write_json(_cache_path(), cache)


def forget_ids(key, url, token, ids):
    """Drop the cached entry of `key` when it holds `ids` for this backend
    and identity (ids the backend refused); True when one was dropped."""
    cache = _read_json(_cache_path())
    if _entry_ids(cache.get(key), url, token) != ids:
        return False
    del cache[key]
    _write_json(_cache_path(), cache)
    return True


def resolve_remote(url, token, key, name, author=AUTHOR, workspace=None, timeout=5):
    body = {"key": key, "name": name, "client": author}
    if workspace:
        body["workspace"] = workspace
    request = urllib.request.Request(
        url + "/api/projects/resolve",
        data=json.dumps(body).encode("utf-8"),
        headers={"Content-Type": "application/json", "X-PCX-Author": author},
        method="POST",
    )
    if token:
        request.add_header("Authorization", "Bearer " + token)
    with urllib.request.urlopen(request, timeout=timeout) as response:
        answer = json.load(response)
    return {"session_id": answer["session_id"], "workspace_id": answer["workspace_id"]}


def event_body(scope, event, author=AUTHOR):
    """The body of POST /api/events: the hook event (Claude Code's JSON, as
    read) with its scope ({"workspace_id"} or {"session_id"})."""
    return {"scope": dict(scope, author=author), "event": event}


def post_event(url, token, scope, event, timeout=5, author=AUTHOR):
    """POST /api/events (see event_body); returns the merged answer of the
    modules, a Claude Code hook answer ({} when they add nothing). Raises on
    a failed request: the hook decides how to fail open."""
    body = event_body(scope, event, author)
    request = urllib.request.Request(
        url.rstrip("/") + "/api/events",
        data=json.dumps(body).encode("utf-8"),
        headers={"Content-Type": "application/json", "X-PCX-Author": author},
        method="POST",
    )
    if token:
        request.add_header("Authorization", "Bearer " + token)
    with urllib.request.urlopen(request, timeout=timeout) as response:
        return json.load(response)


REFUSED = (403, 404)


def post_project_event(context, payload, field, event, timeout=5, author=AUTHOR):
    """post_event for a hook started without ids: `context` is
    global_context's (url, token, ids), the scope `{field: ids[field]}`.
    When the backend refuses the ids (HTTP 403 or 404: a workspace or
    session it does not have, or this key may not use), the cached entry is
    dropped and the project resolved again; the event goes once more with
    ids that differ. Returns (answer, ids it was sent with); raises on a
    failed request, as post_event does."""
    url, token, ids = context
    try:
        return post_event(url, token, {field: ids[field]}, event, timeout, author), ids
    except urllib.error.HTTPError as exc:
        if exc.code not in REFUSED:
            raise
        key = project(project_dir(payload))[0]
        if not forget_ids(key, url, token, ids):
            raise
        fresh = project_ids(payload, url, token, author)
        if not fresh or fresh == ids:
            raise
    return post_event(url, token, {field: fresh[field]}, event, timeout, author), fresh


def added_context(answer):
    """The context an answer adds for the model, or ""."""
    return ((answer or {}).get("hookSpecificOutput") or {}).get("additionalContext") or ""


def project_dir(payload=None):
    payload = payload if isinstance(payload, dict) else {}
    return (
        _clean(payload.get("cwd"))
        or _clean(os.environ.get("CLAUDE_PROJECT_DIR"))
        or os.getcwd()
    )


def project_ids(payload=None, url=None, token=None, author=AUTHOR):
    """{"session_id","workspace_id"} of the project the hook payload is in,
    or None (see the module docs for the order)."""
    try:
        key, name, root = project(project_dir(payload))
        pinned = pinned_ids(root)
        if pinned or not url:
            return pinned
        cached = cached_ids(key, url, token)
        if cached:
            return cached
        ids = resolve_remote(url, token, key, name, author, host_settings(author)["workspace"])
        cache_ids(key, url, token, ids)
        return ids
    except urllib.error.HTTPError as exc:  # fail open, but say why
        _report_refusal(exc)
        return None
    except Exception:  # fail open
        return None


def _report_refusal(exc):
    """Write a refused resolution's status and the server's message to stderr.
    The refusal holds the response open; it is closed here, once read."""
    try:
        with exc:
            body = exc.read().decode("utf-8", "replace").strip()[:MAX_ERROR_CHARS]
    except Exception:
        body = ""
    line = "post-cortex: /api/projects/resolve: HTTP %s" % exc.code
    try:
        print(line + (": " + body if body else ""), file=sys.stderr)
    except Exception:
        pass


def read_payload(stream):
    """The hook's JSON payload, or {}."""
    try:
        payload = json.load(stream)
    except (OSError, ValueError):
        return {}
    return payload if isinstance(payload, dict) else {}


def global_context(script, payload, author=AUTHOR):
    """(url, token, ids) for a hook started without ids (the plugin, a
    global install), or None when it should do nothing: the project runs
    the same hook itself, the client is disabled in config.json, or the
    project cannot be resolved."""
    if shadowed_by_project(script) or not host_settings(author)["enabled"]:
        return None
    url, token, _ = resolve_server()
    if not url:
        return None
    ids = project_ids(payload, url, token, author)
    return (url, token, ids) if ids else None


def ids_line(ids):
    """The line that tells the agent which session and workspace to use."""
    return "[PCX] Session: %s | Workspace: %s" % (ids["session_id"], ids["workspace_id"])


def shadowed_by_project(script):
    """Whether `script` (a plugin hook's __file__) should step aside because
    the project runs the same hook from .claude/hooks (`pcx setup
    --project`): project mode wins, the hook never runs twice."""
    try:
        project_root = _clean(os.environ.get("CLAUDE_PROJECT_DIR"))
        if not project_root:
            return False
        hooks = os.path.join(project_root, ".claude", "hooks")
        name = os.path.basename(script)
        if os.path.realpath(os.path.dirname(os.path.abspath(script))) == os.path.realpath(hooks):
            return False
        if not os.path.exists(os.path.join(hooks, name)):
            return False
        stem = os.path.splitext(name)[0]
        for settings in ("settings.json", "settings.local.json"):
            try:
                with open(os.path.join(project_root, ".claude", settings), encoding="utf-8") as f:
                    if stem in f.read():
                        return True
            except OSError:
                continue
        return False
    except Exception:
        return False
