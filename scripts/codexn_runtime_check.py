"""Exercise packaged helpers locally, without credentials or audio capture."""

from contextlib import contextmanager
import hashlib
import json
import os
from pathlib import Path
import select
import subprocess
import tempfile
import time


@contextmanager
def process(args, env):
    with tempfile.TemporaryFile() as errors:
        child = subprocess.Popen(
            args,
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=errors,
            env=env,
            bufsize=0,
        )
        try:
            yield child
        finally:
            if child.poll() is None:
                child.terminate()
            try:
                child.wait(timeout=5)
            except subprocess.TimeoutExpired:
                child.kill()
                child.wait()


def read_bytes(child, length, deadline):
    data = bytearray()
    while len(data) < length:
        remaining = deadline - time.monotonic()
        if remaining <= 0 or not select.select([child.stdout], [], [], remaining)[0]:
            raise RuntimeError("Packaged runtime did not reply before the deadline")
        chunk = os.read(child.stdout.fileno(), length - len(data))
        if not chunk:
            raise RuntimeError("Packaged runtime exited before replying")
        data.extend(chunk)
    return bytes(data)


def exchange(child, message, byteorder):
    data = json.dumps(message).encode()
    child.stdin.write(len(data).to_bytes(4, byteorder) + data)
    return receive(child, byteorder)


def receive(child, byteorder):
    deadline = time.monotonic() + 30
    length = int.from_bytes(read_bytes(child, 4, deadline), byteorder)
    if not 0 < length <= 64 * 1024 * 1024:
        raise RuntimeError("Invalid packaged runtime response length")
    return json.loads(read_bytes(child, length, deadline))


def executor_info(binary, env):
    with process([str(binary), "exec-server", "--listen", "stdio"], env) as child:
        child.stdin.write(
            json.dumps(
                {
                    "id": 1,
                    "method": "initialize",
                    "params": {"clientName": "codexn-package-check"},
                }
            ).encode()
            + b"\n"
        )
        deadline = time.monotonic() + 30
        line = bytearray()
        while len(line) < 1024 * 1024:
            line.extend(read_bytes(child, 1, deadline))
            if line.endswith(b"\n"):
                response = json.loads(line)
                if response.get("id") == 1:
                    return response["result"]["environmentInfo"]
                line.clear()
        raise RuntimeError("Invalid executor initialize response")


def check_package(package: Path, commit: str, target: str) -> dict:
    env = os.environ.copy()
    for key in (
        "CODEX_API_KEY",
        "OPENAI_API_KEY",
        "OPENAI_BASE_URL",
        "CODEX_THREAD_ID",
        "CODEX_INTERNAL_ORIGINATOR_OVERRIDE",
        "CODEX_MANAGED_PACKAGE_ROOT",
        "CODEX_MANAGED_BY_NPM",
        "CODEX_MANAGED_BY_BUN",
        "CODEX_MANAGED_BY_PNPM",
        "CODEX_MANAGED_BY_VITE_PLUS",
    ):
        env.pop(key, None)
    with tempfile.TemporaryDirectory(prefix="codexn-check-") as home:
        env["CODEX_HOME"] = home
        info = executor_info(package / "bin/codex", env)
        metadata = json.loads((package / "codex-package.json").read_text())
        if info["executorVersion"] != metadata["version"]:
            raise RuntimeError("Main executable did not discover its package layout")
        expected_id = (
            "sha256:" + hashlib.sha256(f"git:{commit}:{target}".encode()).hexdigest()
        )
        if info.get("providerId") != expected_id:
            raise RuntimeError(
                "Main executable build stamp does not match the voice helper"
            )
        with process([str(package / "bin/codex-code-mode-host")], env) as child:
            reply = exchange(
                child,
                {
                    "type": "connection/hello",
                    "supportedVersions": [1],
                    "requiredCapabilities": [],
                    "optionalCapabilities": [],
                },
                "little",
            )
            if reply.get("type") != "connection/ready":
                raise RuntimeError("Code Mode handshake failed")
            opened = exchange(
                child,
                {
                    "type": "operation/request",
                    "id": 1,
                    "request": {"method": "session/open", "sessionId": "package-check"},
                },
                "little",
            )
            if opened.get("result", {}).get("status") != "ok":
                raise RuntimeError("Code Mode session failed to open")
            reply = exchange(
                child,
                {
                    "type": "operation/request",
                    "id": 2,
                    "request": {
                        "method": "session/execute",
                        "sessionId": "package-check",
                        "request": {
                            "tool_call_id": "package-check",
                            "enabled_tools": [],
                            "source": 'text("CODEXN_RUNTIME_OK")',
                            "yield_time_ms": 10000,
                            "max_output_tokens": 1000,
                        },
                    },
                },
                "little",
            )
            if reply.get("result", {}).get("status") != "ok":
                raise RuntimeError("Code Mode execution failed to start")
            reply = receive(child, "little")
            if "CODEXN_RUNTIME_OK" not in json.dumps(reply):
                raise RuntimeError("Code Mode JavaScript execution failed")
        voice = package / "codex-resources/voice/bin/codex-voice-host"
        if voice.exists():
            env.update(
                GST_PLUGIN_PATH="",
                GST_PLUGIN_PATH_1_0="",
                GST_PLUGIN_SYSTEM_PATH="",
                GST_PLUGIN_SYSTEM_PATH_1_0="",
                GST_REGISTRY="/dev/null",
                GST_REGISTRY_UPDATE="no",
                GST_REGISTRY_FORK="no",
            )
            with process([str(voice)], env) as child:
                checks = [
                    ({"type": "hello", "protocol": 1, "buildCommit": commit}, "ready"),
                    ({"type": "initializeRuntime"}, "runtimeReady"),
                    ({"type": "startTransport"}, "offer"),
                    ({"type": "close"}, "closed"),
                ]
                for request, expected in checks:
                    if exchange(child, request, "big").get("type") != expected:
                        raise RuntimeError(f"Voice runtime check failed: {expected}")
                if child.wait(timeout=5) != 0:
                    raise RuntimeError("Voice host did not close cleanly")
        for relative, argument in [
            ("codex-path/rg", "--version"),
            ("codex-resources/zsh/bin/zsh", "--version"),
        ]:
            executable = package / relative
            if executable.exists():
                subprocess.run(
                    [str(executable), argument],
                    env=env,
                    check=True,
                    capture_output=True,
                    timeout=30,
                )
    return {
        "main_build_identity": expected_id,
        "code_mode_javascript": True,
        "voice_runtime_and_offer": voice.exists(),
        "audio_devices_opened": False,
    }
