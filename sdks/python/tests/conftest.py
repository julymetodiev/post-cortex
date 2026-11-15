# Copyright (c) 2026 Chronicle Labs Ltd.
# MIT License
"""A throwaway post-cortex daemon for the tests.

Uses ``PCX_BIN`` or the workspace's ``target/debug/pcx`` (``cargo build -p
post-cortex-daemon --bin pcx``); tests are skipped when neither exists. Each daemon
gets its own HOME and RocksDB directory; the Hugging Face cache is shared so the
embedding model is downloaded once.
"""

from __future__ import annotations

import os
import socket
import subprocess
import tempfile
import time
from pathlib import Path
from typing import Callable, Iterator, Optional, TypeVar

import pytest

from post_cortex import Client, NotFound, PostCortexError

REPO = Path(__file__).resolve().parents[3]


def _binary() -> Optional[Path]:
    path = Path(os.environ.get("PCX_BIN") or REPO / "target" / "debug" / "pcx")
    return path if path.exists() else None


def _free_port() -> int:
    with socket.socket() as s:
        s.bind(("127.0.0.1", 0))
        port: int = s.getsockname()[1]
        return port


class Daemon:
    def __init__(self, token: Optional[str] = None):
        binary = _binary()
        if binary is None:
            pytest.skip("no pcx binary (set PCX_BIN or cargo build -p post-cortex-daemon --bin pcx)")
        self.dir = tempfile.TemporaryDirectory(prefix="pcx-sdk-")
        home = Path(self.dir.name)
        self.port, self.grpc_port = _free_port(), _free_port()
        env = {
            **os.environ,
            "HOME": str(home),
            "HF_HOME": os.environ.get("HF_HOME", str(Path.home() / ".cache" / "huggingface")),
            "PC_DATA_DIR": str(home / "data"),
            "PC_STORAGE_BACKEND": "rocksdb",
            "PC_HOST": "127.0.0.1",
            "PC_PORT": str(self.port),
            "PC_GRPC_PORT": str(self.grpc_port),
            "RUST_LOG": "warn",
        }
        env.pop("PC_AUTH_TOKEN", None)
        if token:
            env["PC_AUTH_TOKEN"] = token
        self.log = open(home / "daemon.log", "w")
        self.proc = subprocess.Popen(
            [str(binary), "start"], env=env, stdout=self.log, stderr=subprocess.STDOUT
        )
        self.target = f"127.0.0.1:{self.grpc_port}"
        self._wait(token)

    def _wait(self, token: Optional[str]) -> None:
        deadline = time.time() + 120
        while time.time() < deadline:
            if self.proc.poll() is not None:
                raise RuntimeError(f"pcx exited: {Path(self.log.name).read_text()[-2000:]}")
            try:
                with Client(self.target, token=token, timeout=2) as c:
                    if c.health().healthy:
                        break
            except PostCortexError:
                pass
            time.sleep(0.5)
        else:
            raise RuntimeError("pcx did not come up in 120 s")
        # The embedding model loads on first use (seconds in a release build, a minute or
        # more in a debug one); load it once here so no test runs into its deadline.
        with Client(self.target, token=token, timeout=600) as c:
            try:
                c.search("warm up", limit=1)
            except PostCortexError:
                pass

    def stop(self) -> None:
        self.proc.terminate()
        try:
            self.proc.wait(timeout=20)
        except subprocess.TimeoutExpired:
            self.proc.kill()
        self.log.close()
        self.dir.cleanup()


@pytest.fixture(scope="session")
def daemon() -> Iterator[Daemon]:
    d = Daemon()
    yield d
    d.stop()


@pytest.fixture()
def pcx(daemon: Daemon) -> Iterator[Client]:
    with Client(daemon.target, author="sdk-test", token="") as c:
        yield c


T = TypeVar("T")


def eventually(check: Callable[[], Optional[T]], timeout: float = 30.0, interval: float = 0.2) -> T:
    """Retry ``check`` until it returns a truthy value (background work on the daemon)."""
    deadline = time.time() + timeout
    while True:
        try:
            value = check()
        except NotFound:
            value = None
        if value:
            return value
        if time.time() > deadline:
            raise AssertionError("condition not met in time")
        time.sleep(interval)
