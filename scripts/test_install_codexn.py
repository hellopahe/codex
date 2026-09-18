"""Regression coverage for complete, atomic custom CLI installation."""

import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

import install_codexn as installer


class InstallTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.baseline = self.root / "official"
        self.home = self.root / "home"
        self.commit = "a" * 40
        self.binary = self.root / "custom"
        self.voice = self.root / "voice"
        self.binary.write_text("custom main")
        self.voice.write_text("custom voice")
        manifest = {
            "layoutVersion": 1,
            "version": "0.155.0",
            "target": "aarch64-apple-darwin",
            "variant": "codex",
            "entrypoint": "bin/codex",
            "resourcesDir": "codex-resources",
            "pathDir": "codex-path",
        }
        files = {
            "codex-package.json": json.dumps(manifest),
            "bin/codex": "official main",
            "bin/codex-code-mode-host": "v8 host",
            "codex-path/rg": "rg",
            "codex-resources/zsh/bin/zsh": "zsh",
            "codex-resources/future/new-resource": "must survive",
            "codex-resources/voice/bin/codex-voice-host": "official voice",
            "codex-resources/voice/lib/example.dylib": "native library",
            "codex-resources/voice/runtime.json": json.dumps(
                {
                    "sourceManifestSha256": installer.digest(
                        installer.REPO / "third_party/voice/sources.json"
                    )
                }
            ),
        }
        for name, content in files.items():
            path = self.baseline / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text(content)
            path.chmod(0o755)
        voice_manifest = {
            "buildCommit": "b" * 40,
            "sha256": {
                name: installer.digest(self.baseline / name)
                for name in files
                if name.startswith("codex-resources/voice/") or name == "bin/codex"
            },
        }
        (self.baseline / "codex-resources/voice/manifest.json").write_text(
            json.dumps(voice_manifest)
        )
        self.output = patch.object(
            installer.subprocess, "check_output", side_effect=self.command
        )
        self.output.start()
        self.addCleanup(self.output.stop)
        self.checks = patch.object(
            installer, "check_package", return_value={"passed": True}
        )
        self.check_mock = self.checks.start()
        self.addCleanup(self.checks.stop)

    def command(self, args, **kwargs):
        return self.commit if args[-1] == "--build-commit" else "codex-cli 0.155.0"

    def install(self):
        return installer.install(
            self.binary, self.baseline, self.home, self.voice, self.commit
        )

    def test_complete_copy_preserves_official_and_future_resources(self):
        before = installer.inventory(self.baseline)
        self.install()
        current = self.home / ".local/lib/codexn/current"
        self.assertEqual(
            (current / "codex-resources/future/new-resource").read_text(),
            "must survive",
        )
        self.assertEqual(installer.inventory(self.baseline), before)
        self.assertEqual(
            installer.validate_voice_manifest(current)["buildCommit"], self.commit
        )
        self.assertEqual(self.check_mock.call_count, 1)

    def test_missing_code_mode_host_preserves_previous_install(self):
        self.install()
        current = self.home / ".local/lib/codexn/current"
        previous = current.readlink()
        (self.baseline / "bin/codex-code-mode-host").unlink()
        with self.assertRaisesRegex(
            RuntimeError, "Missing official package executable"
        ):
            self.install()
        self.assertEqual(current.readlink(), previous)

    def test_failed_runtime_check_preserves_previous_install(self):
        self.install()
        current = self.home / ".local/lib/codexn/current"
        previous = current.readlink()
        self.binary.write_text("next build")
        self.check_mock.side_effect = RuntimeError("runtime mismatch")
        with self.assertRaisesRegex(RuntimeError, "runtime mismatch"):
            self.install()
        self.assertEqual(current.readlink(), previous)

    def test_corrupt_existing_release_is_rejected(self):
        self.install()
        (self.home / ".local/lib/codexn/current/codex-path/rg").write_text("corrupt")
        with self.assertRaisesRegex(RuntimeError, "Existing release is inconsistent"):
            self.install()

    def test_version_and_voice_mismatch_are_rejected(self):
        with patch.object(
            installer.subprocess, "check_output", return_value="codex-cli 0.154.0"
        ):
            with self.assertRaisesRegex(RuntimeError, "Version mismatch"):
                self.install()
        with patch.object(
            installer.subprocess,
            "check_output",
            side_effect=["codex-cli 0.155.0", "b" * 40],
        ):
            with self.assertRaisesRegex(RuntimeError, "different source commit"):
                self.install()
        self.assertFalse((self.home / ".local/lib/codexn/current").exists())

    def test_corrupt_official_voice_library_is_rejected(self):
        (self.baseline / "codex-resources/voice/lib/example.dylib").write_text("broken")
        with self.assertRaisesRegex(RuntimeError, "checksum mismatch"):
            self.install()

    def test_missing_packaged_zsh_is_rejected(self):
        (self.baseline / "codex-resources/zsh/bin/zsh").unlink()
        with self.assertRaisesRegex(
            RuntimeError, "Missing official package executable"
        ):
            self.install()

    def test_internal_absolute_links_cannot_write_back_into_official_package(self):
        source = self.baseline / "bin/codex"
        content = source.read_bytes()
        (self.baseline / "linked-main").symlink_to(source)
        with self.assertRaisesRegex(RuntimeError, "symbolic link"):
            self.install()
        self.assertEqual(source.read_bytes(), content)


if __name__ == "__main__":
    unittest.main()
