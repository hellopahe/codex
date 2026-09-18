#!/usr/bin/env python3
"""Install a custom entrypoint with the complete, matching official runtime package."""

import argparse
import hashlib
import json
import os
import platform
import re
import shutil
import subprocess
import tempfile
from pathlib import Path

from codexn_runtime_check import check_package

REPO = Path(__file__).resolve().parent.parent


def digest(path: Path) -> str:
    with path.open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def inventory(root: Path) -> dict:
    result = {}
    for path in sorted(root.rglob("*")):
        relative = path.relative_to(root).as_posix()
        if path.is_symlink():
            # Canonical upstream packages contain regular files. In particular,
            # never copy an absolute link that could write back into the source.
            raise RuntimeError(f"Package contains a symbolic link: {relative}")
        elif path.is_file():
            result[relative] = {
                "sha256": digest(path),
                "executable": bool(path.stat().st_mode & 0o111),
            }
    return result


def official_package() -> Path:
    command = shutil.which("codex")
    if not command:
        raise RuntimeError("Cannot find official codex; pass --official-package PATH")
    executable = Path(command).resolve()
    arch = "arm64" if platform.machine().lower() in ("arm64", "aarch64") else "x64"
    system = {"Darwin": "darwin", "Linux": "linux"}.get(platform.system())
    if not system:
        raise RuntimeError("This installer supports Unix runtime packages")
    for parent in executable.parents:
        if (parent / "codex-package.json").is_file():
            return parent
        for modules in (parent / "node_modules" / "@openai", parent.parent):
            manifests = list(
                (modules / f"codex-{system}-{arch}" / "vendor").glob(
                    "*/codex-package.json"
                )
            )
            if len(manifests) == 1:
                return manifests[0].parent
    raise RuntimeError(
        "Cannot locate the official runtime package; pass --official-package PATH"
    )


def atomic_link(path: Path, target: str) -> None:
    temporary = path.with_name(path.name + ".new")
    temporary.unlink(missing_ok=True)
    temporary.symlink_to(target)
    os.replace(temporary, path)


def validate_voice_manifest(package: Path) -> dict:
    manifest = json.loads((package / "codex-resources/voice/manifest.json").read_text())
    for name, expected in manifest["sha256"].items():
        path = package / name
        if (
            not path.resolve().is_relative_to(package.resolve())
            or digest(path) != expected
        ):
            raise RuntimeError(f"Voice package checksum mismatch: {name}")
    return manifest


def install(
    binary: Path, baseline: Path, home: Path, voice_host: Path, commit: str
) -> dict:
    binary, baseline = binary.resolve(), baseline.resolve()
    manifest = json.loads((baseline / "codex-package.json").read_text())
    if manifest.get("layoutVersion") != 1 or manifest.get("entrypoint") != "bin/codex":
        raise RuntimeError("Unsupported official package layout")
    version = (
        subprocess.check_output([str(binary), "--version"], text=True, timeout=30)
        .strip()
        .split()[1]
    )
    if manifest["version"] != version:
        raise RuntimeError(
            f"Version mismatch: custom {version}, official {manifest['version']}"
        )
    if not re.fullmatch(r"[0-9a-f]{40}", commit):
        raise RuntimeError("A full source commit is required for a distributable build")
    for name in (
        "bin/codex",
        "bin/codex-code-mode-host",
        "codex-path/rg",
        "codex-resources/zsh/bin/zsh",
        "codex-resources/voice/bin/codex-voice-host",
    ):
        if not (baseline / name).is_file() or not os.access(baseline / name, os.X_OK):
            raise RuntimeError(f"Missing official package executable: {name}")
    voice_manifest = validate_voice_manifest(baseline)
    runtime = json.loads((baseline / "codex-resources/voice/runtime.json").read_text())
    if runtime["sourceManifestSha256"] != digest(
        REPO / "third_party/voice/sources.json"
    ):
        raise RuntimeError(
            "Official voice libraries differ from this source tree's pinned native dependencies"
        )
    expected = inventory(baseline)
    binary_sha = digest(binary)
    runtime_sha = hashlib.sha256(
        json.dumps(expected, sort_keys=True).encode()
    ).hexdigest()
    root = home / ".local/lib/codexn"
    releases = root / "releases"
    releases.mkdir(parents=True, exist_ok=True)
    voice_sha = digest(voice_host)
    release = (
        releases / f"{version}-{binary_sha[:12]}-{runtime_sha[:12]}-{voice_sha[:12]}"
    )
    with tempfile.TemporaryDirectory(prefix=".staging-", dir=releases) as staging:
        package = Path(staging) / "package"
        shutil.copytree(baseline, package, symlinks=True)
        shutil.copy2(binary, package / "bin/codex")
        voice_relative = "codex-resources/voice/bin/codex-voice-host"
        shutil.copy2(voice_host, package / voice_relative)
        actual_commit = subprocess.check_output(
            [str(package / voice_relative), "--build-commit"], text=True, timeout=30
        ).strip()
        if actual_commit != commit:
            raise RuntimeError(
                "Voice helper was compiled from a different source commit"
            )
        voice_manifest["buildCommit"] = commit
        voice_manifest["sha256"].update(
            {"bin/codex": binary_sha, voice_relative: voice_sha}
        )
        voice_manifest_relative = "codex-resources/voice/manifest.json"
        (package / voice_manifest_relative).write_text(
            json.dumps(voice_manifest, indent=2) + "\n"
        )
        validate_voice_manifest(package)
        actual = inventory(package)
        if set(actual) != set(expected):
            raise RuntimeError("Incomplete runtime package copy")
        for name, info in expected.items():
            if (
                name not in ("bin/codex", voice_relative, voice_manifest_relative)
                and actual[name] != info
            ):
                raise RuntimeError(
                    f"Runtime component differs from official package: {name}"
                )
        if actual["bin/codex"]["sha256"] != binary_sha:
            raise RuntimeError("Custom executable failed its copy checksum")
        runtime_checks = check_package(package, commit, manifest["target"])
        receipt = {
            "version": version,
            "target": manifest["target"],
            "custom_binary_sha256": binary_sha,
            "source_commit": commit,
            "custom_voice_host_sha256": voice_sha,
            "official_runtime_source": str(baseline),
            "official_inventory": expected,
            "identical_runtime_files": len(expected) - 3,
            "checks": runtime_checks,
        }
        (package / "codexn-build-info.json").write_text(
            json.dumps(receipt, indent=2) + "\n"
        )
        if release.exists():
            installed = inventory(release)
            if set(installed) != set(actual) | {"codexn-build-info.json"}:
                raise RuntimeError(
                    "Existing release contains an unexpected file inventory"
                )
            for name, info in actual.items():
                if installed.get(name) != info:
                    raise RuntimeError(f"Existing release is inconsistent: {name}")
        else:
            os.replace(package, release)
    atomic_link(root / "current", str(release.relative_to(root)))
    # Preserve old helper paths for existing processes; new launches discover the canonical layout.
    atomic_link(root / "codex-code-mode-host", "current/bin/codex-code-mode-host")
    atomic_link(root / "codex", "current/bin/codex")
    atomic_link(root / "build-info.json", "current/codexn-build-info.json")
    launcher = home / ".local/bin/codexn"
    launcher.parent.mkdir(parents=True, exist_ok=True)
    temporary = launcher.with_name("codexn.new")
    temporary.write_text(
        "#!/bin/sh\n"
        'export CODEX_NETWORK_MONITOR="${CODEX_NETWORK_MONITOR:-1}"\n'
        "unset CODEX_MANAGED_BY_NPM CODEX_MANAGED_BY_BUN CODEX_MANAGED_BY_PNPM CODEX_MANAGED_BY_VITE_PLUS CODEX_MANAGED_PACKAGE_ROOT\n"
        'exec "$HOME/.local/lib/codexn/current/bin/codex" "$@"\n'
    )
    temporary.chmod(0o755)
    os.replace(temporary, launcher)
    return receipt


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--from-binary", type=Path)
    parser.add_argument("--official-package", type=Path)
    parser.add_argument("--voice-host", type=Path)
    parser.add_argument("--build-commit")
    args = parser.parse_args()
    commit = (
        args.build_commit
        or subprocess.check_output(
            ["git", "rev-parse", "HEAD"], cwd=REPO, text=True
        ).strip()
    )
    if bool(args.from_binary) != bool(args.voice_host):
        parser.error("--from-binary and --voice-host must be supplied together")
    if args.from_binary is None:
        env = os.environ.copy()
        env["STABLE_GIT_COMMIT"] = commit
        subprocess.run(
            [
                "cargo",
                "build",
                "--locked",
                "--release",
                "-p",
                "codex-cli",
                "--bin",
                "codex",
            ],
            cwd=REPO / "codex-rs",
            env=env,
            check=True,
        )
        subprocess.run(
            ["bazel", "build", "-c", "opt", "//codex-rs/voice-host:codex-voice-host"],
            cwd=REPO,
            check=True,
        )
    binary = args.from_binary or REPO / "codex-rs/target/release/codex"
    voice = args.voice_host or REPO / "bazel-bin/codex-rs/voice-host/codex-voice-host"
    receipt = install(
        binary, args.official_package or official_package(), Path.home(), voice, commit
    )
    print(
        f"Installed codexn {receipt['version']}; {receipt['identical_runtime_files']} runtime files verified identical to official package."
    )


if __name__ == "__main__":
    main()
