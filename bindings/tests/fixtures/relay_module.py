# Copyright (c) 2026 Julius ML
# Licensed under the MIT License. See LICENSE at the workspace root.

"""The bindings' test module in Python (tests/bindings.rs): Relay, which
calls Helper through its host."""

import asyncio
import os
import socket
import subprocess
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

from relay.v1 import relay_pb2 as pb  # noqa: E402
from pcx_module import Effect, Module, ModuleError  # noqa: E402

module = Module(
    id="relay",
    version="0.1.0",
    proto="relay/v1/relay.proto",
    description="Relays to the helper: the bindings' test module.",
    uses=["helper"],
)


@module.tool("/test.relay.v1.Relay/Upper", name="relay_upper", effect=Effect.READ)
async def upper(ctx, request: pb.Text) -> pb.Text:
    if not request.text:
        raise ModuleError("invalid_argument", "there is no text to relay")
    answer = await ctx.host.call("/test.relay.v1.Helper/Upper", request)
    return pb.Text(text=f"{answer.text} for {ctx.author}")


@module.tool("/test.relay.v1.Relay/Wait", effect=Effect.READ)
async def wait(ctx, request: pb.Text) -> pb.Text:
    await asyncio.sleep(request.wait_ms / 1000)
    return pb.Text(text="cancelled" if ctx.cancelled else "waited")


held = []


@module.tool("/test.relay.v1.Relay/Probe", effect=Effect.READ)
async def probe(ctx, request: pb.Text) -> pb.Text:
    action, _, arg = request.text.partition(":")
    try:
        if action == "spawn":
            subprocess.run(["/bin/echo"], check=True, capture_output=True)
        elif action == "connect":
            socket.create_connection(("1.1.1.1", 53), timeout=2).close()
        elif action == "read":
            with open(arg, "rb") as f:
                f.read(1)
        elif action == "write":
            with open(arg, "w") as f:
                f.write("x")
        elif action == "alloc":
            held.append(b"x" * (int(arg) * 1024 * 1024))
            await asyncio.sleep(5)
        elif action == "files":
            opened = []
            try:
                while True:
                    opened.append(open("/dev/null"))
            except OSError:
                return pb.Text(text=str(len(opened)))
            finally:
                for f in opened:
                    f.close()
        elif action == "pid":
            return pb.Text(text=str(os.getpid()))
    except TimeoutError:
        return pb.Text(text="timed out")
    except OSError as e:
        return pb.Text(text=f"refused: {e}")
    return pb.Text(text="allowed")


if __name__ == "__main__":
    module.run()
