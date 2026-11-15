# Copyright (c) 2026 Julius ML
# Licensed under the MIT License. See LICENSE at the workspace root.

"""A pcx module in Python (docs/modules/dynamic.md, the bindings' API).

The Rust core serves the module to its host; this layer declares the module
(its proto, its tools, what it uses) and runs its handlers on worker
threads, each with its own event loop, on free-threaded CPython::

    from pcx_module import Module, Effect
    from greet.v1 import greet_pb2 as pb

    module = Module(id="greet-py", version="0.1.0", proto="greet/v1/greet.proto",
                    description="Says hello.")

    @module.tool("/example.greet.v1.Greeter/Greet", name="greet", effect=Effect.WRITE)
    async def greet(ctx, request: pb.GreetRequest) -> pb.GreetResponse:
        return pb.GreetResponse(text=f"Hello, {request.name}")

    module.run()

Only message bytes cross to the core; the module's messages are its own
generated code (``protobuf``), never the module protocol's.
"""

from __future__ import annotations

import asyncio
import enum
import inspect
import logging
import sys
import threading
from dataclasses import dataclass
from typing import Any, Awaitable, Callable

from google.protobuf import descriptor_pb2, descriptor_pool, message, message_factory

from . import _core

__all__ = ["Context", "Effect", "Host", "Module", "ModuleError", "Visibility"]


class Effect(enum.Enum):
    """What a call of an RPC does."""

    READ = "read"
    """Changes nothing, in pcx or outside."""
    WRITE = "write"
    """Changes state inside pcx only."""
    EXTERNAL = "external"
    """Acts outside pcx."""


class Visibility(enum.Enum):
    """Who may call a tool."""

    BOTH = "both"
    """An agent and the module's views."""
    MODEL = "model"
    """An agent only."""
    APP = "app"
    """The module's views only."""


class ModuleError(Exception):
    """A failed call: its ``kind`` (``not_found``, ``denied``, ``invalid_argument``,
    ...) is what a caller acts on, its ``message`` is for a person."""

    def __init__(self, kind: str, message: str) -> None:
        super().__init__(kind, message)
        self.kind = kind
        self.message = message

    def __str__(self) -> str:
        return f"{self.kind}: {self.message}"


def _failed(error: _core.ModuleError) -> ModuleError:
    kind, text = error.args
    return ModuleError(kind, text)


Handler = Callable[["Context", message.Message], Awaitable[message.Message]]


@dataclass(frozen=True)
class _Tool:
    path: str
    name: str
    effect: Effect
    visibility: Visibility
    request: type[message.Message]
    handler: Handler


def _method(pool: descriptor_pool.DescriptorPool, path: str):
    """The method of ``path``, ``/<package>.<Service>/<Method>``."""
    parts = path.split("/")
    if len(parts) != 3 or parts[0] or not parts[1] or not parts[2]:
        raise ValueError(f"{path} is not a method's path: /<package>.<Service>/<Method>")
    try:
        return pool.FindMethodByName(f"{parts[1]}.{parts[2]}")
    except KeyError:
        raise ValueError(f"no method {path} among the messages imported") from None


def _descriptors(pool: descriptor_pool.DescriptorPool, proto: str):
    """The file ``proto`` and every file it imports, imported first, as a
    ``FileDescriptorSet``."""
    try:
        root = pool.FindFileByName(proto)
    except KeyError:
        raise ValueError(f"{proto} is not imported: import its generated module first") from None
    files: list[Any] = []
    seen: set[str] = set()

    def add(file) -> None:
        if file.name in seen:
            return
        seen.add(file.name)
        for dependency in file.dependencies:
            add(dependency)
        files.append(file)

    add(root)
    out = descriptor_pb2.FileDescriptorSet()
    for file in files:
        out.file.add().ParseFromString(file.serialized_pb)
    return out.SerializeToString()


class Host:
    """The module's handle on its host, within one call."""

    def __init__(self, core: _core.Core, call: int, pool: descriptor_pool.DescriptorPool) -> None:
        self._core = core
        self._call = call
        self._pool = pool

    async def call(self, path: str, request: message.Message) -> message.Message:
        """Call ``path`` of a module this one uses (its ``uses``), in this call's
        scope: its response, a message of the method's output type, whose
        generated module must be imported."""
        response = message_factory.GetMessageClass(_method(self._pool, path).output_type)
        try:
            answer = await asyncio.to_thread(
                self._core.host_call, self._call, path, request.SerializeToString()
            )
        except _core.ModuleError as e:
            raise _failed(e) from None
        return response.FromString(answer)


class Context:
    """One call: who it is for, its deadline, the host, the settings."""

    def __init__(self, module: Module, call: _core.Call) -> None:
        self._module = module
        self._call = call
        self.host = Host(module._core, call.id, module._pool)
        """The module's handle on its host."""
        self.log = logging.getLogger(module.id)
        """The module's log, which its host keeps."""

    @property
    def author(self) -> str | None:
        """Who calls (a person, an agent, a module); none: unknown."""
        return self._call.author

    @property
    def workspace(self) -> str | None:
        """The workspace it is for; none: the caller's default."""
        return self._call.workspace

    @property
    def caller(self) -> str | None:
        """The module that calls; none: a person or an agent."""
        return self._call.caller

    @property
    def remaining(self) -> float:
        """Seconds left before its deadline, as handed over."""
        return self._call.remaining

    @property
    def cancelled(self) -> bool:
        """Whether its caller gave up (its deadline passed)."""
        return not self._module._core.waits(self._call.id)

    @property
    def settings(self) -> message.Message | None:
        """The module's settings, a message of its ``config``; none without."""
        return self._module._settings()

    async def publish(self, name: str, event: message.Message) -> str:
        """Emit the event ``name`` the module declares (its ``emits``): its id."""
        try:
            return await asyncio.to_thread(
                self._module._core.publish,
                self._call.id,
                name,
                event.DESCRIPTOR.full_name,
                event.SerializeToString(),
            )
        except _core.ModuleError as e:
            raise _failed(e) from None


class Module:
    """A module: what it declares, its tools' handlers, and its run."""

    def __init__(
        self,
        *,
        id: str,
        version: str,
        proto: str,
        description: str = "",
        uses: list[str] | tuple[str, ...] = (),
        emits: list[tuple[str, type[message.Message], str]] | tuple = (),
        follows: list[str] | tuple[str, ...] = (),
        config: type[message.Message] | None = None,
        concurrency: int = 8,
    ) -> None:
        """``proto`` is the name of its proto file as its generated module
        registers it (``greet/v1/greet.proto``), imported before; ``emits``
        its events as (name, message type, description); ``config`` the
        message of its settings; ``concurrency`` the calls it runs at once."""
        if concurrency < 1:
            raise ValueError("a module runs at least one call at once")
        self.id = id
        self.version = version
        self.description = description
        self.proto = proto
        self.uses = list(uses)
        self.emits = list(emits)
        self.follows = list(follows)
        self.config = config
        self.concurrency = concurrency
        self._pool = descriptor_pool.Default()
        self._tools: dict[str, _Tool] = {}
        self._core: _core.Core | None = None
        self._config: message.Message | None = None
        self._config_read = threading.Lock()

    def tool(
        self,
        path: str,
        *,
        name: str = "",
        effect: Effect,
        visibility: Visibility = Visibility.BOTH,
    ) -> Callable[[Handler], Handler]:
        """Declare the RPC ``path`` of its proto, handled by the decorated
        ``async def handler(ctx, request) -> response``; ``name`` its tool's
        (empty: an RPC only its views or other modules call)."""

        def declare(handler: Handler) -> Handler:
            if not inspect.iscoroutinefunction(handler):
                raise TypeError(f"the handler of {path} is not an async def")
            if path in self._tools:
                raise ValueError(f"{path} has a handler already")
            method = _method(self._pool, path)
            self._tools[path] = _Tool(
                path=path,
                name=name,
                effect=effect,
                visibility=visibility,
                request=message_factory.GetMessageClass(method.input_type),
                handler=handler,
            )
            return handler

        return declare

    def _declaring(self) -> dict[str, Any]:
        # The services it serves: those of its RPCs.
        services = list(dict.fromkeys(path.split("/")[1] for path in self._tools))
        return {
            "id": self.id,
            "version": self.version,
            "description": self.description,
            "services": services,
            "descriptors": _descriptors(self._pool, self.proto),
            "rpcs": [
                (t.path, t.name, t.effect.value, t.visibility.value) for t in self._tools.values()
            ],
            "uses": self.uses,
            "emits": [(n, m.DESCRIPTOR.full_name, d) for (n, m, d) in self.emits],
            "follows": self.follows,
            "config_type": self.config.DESCRIPTOR.full_name if self.config else "",
        }

    def _settings(self) -> message.Message | None:
        if self.config is None:
            return None
        with self._config_read:
            if self._config is None:
                kind, body = self._core.config()
                if kind and kind != self.config.DESCRIPTOR.full_name:
                    raise ModuleError(
                        "internal",
                        f"the host gave settings of {kind}, not {self.config.DESCRIPTOR.full_name}",
                    )
                self._config = self.config.FromString(body)
            return self._config

    async def _answer(self, call: _core.Call) -> None:
        core = self._core
        tool = self._tools.get(call.method)
        if tool is None:
            core.fail(call.id, "not_found", f"{self.id} serves no method {call.method}")
            return
        try:
            request = tool.request.FromString(call.body)
        except message.DecodeError as e:
            core.fail(call.id, "invalid_argument", f"the request is not a {tool.request.DESCRIPTOR.full_name}: {e}")
            return
        try:
            response = await tool.handler(Context(self, call), request)
            core.complete(call.id, response.SerializeToString())
        except ModuleError as e:
            core.fail(call.id, e.kind, e.message)
        except Exception as e:  # a defect of the handler: the call fails, the module goes on
            logging.getLogger(self.id).exception("%s failed", call.method)
            core.fail(call.id, "internal", f"{call.method}: {e!r}")

    def _work(self) -> None:
        loop = asyncio.new_event_loop()
        try:
            while (call := self._core.next_call()) is not None:
                loop.run_until_complete(self._answer(call))
        finally:
            loop.close()

    def run(self) -> None:
        """Serve the module on the socket its host gives, until the host stops it."""
        logging.basicConfig(stream=sys.stderr, level=logging.INFO, format="%(levelname)s %(name)s %(message)s")
        if sys._is_gil_enabled():
            # A package that does not declare itself safe without it turned it
            # on for the whole process; the host's check shows this to a person.
            print("pcx_module: the GIL is on: this module's threads run one at a time", file=sys.stderr, flush=True)
        self._core = _core.Core(self._declaring())
        workers = [
            threading.Thread(target=self._work, name=f"{self.id}-{n}", daemon=True)
            for n in range(self.concurrency)
        ]
        for worker in workers:
            worker.start()
        try:
            self._core.serve()
        except _core.ModuleError as e:
            raise _failed(e) from None
        for worker in workers:
            worker.join()
