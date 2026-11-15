# Copyright (c) 2026 Julius ML
# Licensed under the MIT License. See LICENSE at the workspace root.

"""The greet example in Python, on the bindings (docs/modules/dynamic.md).

The host runs it as a process module: ``python3.15t module.py``.
"""

import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

from greet.v1 import greet_pb2 as pb  # noqa: E402
from pcx_module import Effect, Module  # noqa: E402

module = Module(
    id="greet-py",
    version="0.1.0",
    proto="greet/v1/greet.proto",
    description="Says hello: the greet example, in Python.",
)


@module.tool("/example.greet.v1.Greeter/Greet", name="greet", effect=Effect.READ)
async def greet(ctx, request: pb.GreetRequest) -> pb.GreetResponse:
    return pb.GreetResponse(text=f"Hello, {request.name}")


if __name__ == "__main__":
    module.run()
