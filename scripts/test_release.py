"""Release tests run without credentials, network access, or Git mutations."""

import contextlib
import io
import os
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

import release


class VersionTests(unittest.TestCase):
    def test_bumps_reset_lower_components(self):
        self.assertEqual(release.next_version("1.2.3", "none"), "1.2.3")
        self.assertEqual(release.next_version("1.2.3", "patch"), "1.2.4")
        self.assertEqual(release.next_version("1.2.3", "minor"), "1.3.0")
        self.assertEqual(release.next_version("1.2.3", "major"), "2.0.0")

    def test_rejects_invalid_and_overflowing_versions(self):
        for version in ("v0.1.0", "01.0.0", "0.1", "0.1.0\nx", "0.1.0-rc1", "-1.0.0"):
            with self.subTest(version=version), self.assertRaises(RuntimeError):
                release.version_parts(version)
        with self.assertRaises(RuntimeError):
            release.next_version(f"0.0.{2**64 - 1}", "patch")
        with self.assertRaises(RuntimeError):
            release.next_version("0.1.0", "manual")

    def test_updates_only_own_package_and_installation_snippet(self):
        original = Path.cwd()
        with tempfile.TemporaryDirectory() as temporary:
            try:
                os.chdir(temporary)
                Path("comparison").mkdir()
                Path("Cargo.toml").write_text('[package]\nname = "zsyn"\nversion = "0.1.0"\n'
                                              '\n[dependencies]\n', encoding="utf-8")
                lock = ('version = 3\n\n[[package]]\nname = "zsyn"\nversion = "0.1.0"\n'
                        '\n[[package]]\nname = "typst-syntax"\nversion = "0.15.1"\n')
                Path("Cargo.lock").write_text(lock, encoding="utf-8")
                Path("comparison/Cargo.lock").write_text(lock, encoding="utf-8")
                Path("README.md").write_text('zsyn = "0.1.0"\n'
                                            'zsyn = { version = "0.1.0", features = ["alloc"] }\n'
                                            'Measured version 0.1.0.\n', encoding="utf-8")
                release.set_version("0.2.0")
                self.assertEqual(release.package_version(), "0.2.0")
                for name in ("Cargo.lock", "comparison/Cargo.lock"):
                    text = Path(name).read_text(encoding="utf-8")
                    self.assertIn('name = "zsyn"\nversion = "0.2.0"', text)
                    self.assertIn('name = "typst-syntax"\nversion = "0.15.1"', text)
                text = Path("README.md").read_text(encoding="utf-8")
                self.assertIn('zsyn = "0.2.0"', text)
                self.assertIn('version = "0.2.0", features', text)
                self.assertIn("Measured version 0.1.0.", text)
            finally:
                os.chdir(original)


class TrustTests(unittest.TestCase):
    def test_rejects_pr_fork_and_nondefault_branch(self):
        valid = {"GITHUB_EVENT_NAME": "workflow_dispatch", "GITHUB_REPOSITORY": release.REPOSITORY,
                 "GITHUB_REF": "refs/heads/main"}
        cases = ({"GITHUB_EVENT_NAME": "pull_request"}, {"GITHUB_REPOSITORY": "other/zsyn"},
                 {"GITHUB_REF": "refs/heads/development"}, {"GITHUB_REF": "refs/tags/v0.1.0"})
        for changes in cases:
            with self.subTest(changes=changes), patch.dict(os.environ, valid | changes, clear=True), \
                    patch.object(release, "run", return_value="main"), \
                    patch.object(release, "git") as git, self.assertRaises(RuntimeError):
                release.trusted_branch()
            git.assert_not_called()

    def test_allows_only_default_branch_push_and_manual_dispatch(self):
        for event in ("push", "workflow_dispatch"):
            env = {"GITHUB_EVENT_NAME": event, "GITHUB_REPOSITORY": release.REPOSITORY,
                   "GITHUB_REF": "refs/heads/main"}
            with self.subTest(event=event), patch.dict(os.environ, env, clear=True), \
                    patch.object(release, "run", return_value="main"), \
                    patch.object(release, "git") as git:
                self.assertEqual(release.trusted_branch(), "main")
            git.assert_called_once_with("fetch", "origin", "main", "--tags")

    def test_rejects_pr_target_and_fork_pushes(self):
        cases = (
            {"GITHUB_EVENT_NAME": "pull_request_target", "GITHUB_REPOSITORY": release.REPOSITORY,
             "GITHUB_REF": "refs/heads/main"},
            {"GITHUB_EVENT_NAME": "push", "GITHUB_REPOSITORY": "other/zsyn",
             "GITHUB_REF": "refs/heads/main"},
            {"GITHUB_EVENT_NAME": "push", "GITHUB_REPOSITORY": release.REPOSITORY,
             "GITHUB_REF": "refs/heads/feature"},
        )
        for env in cases:
            with self.subTest(env=env), patch.dict(os.environ, env, clear=True), \
                    patch.object(release, "run", return_value="main"), \
                    patch.object(release, "git") as git, self.assertRaises(RuntimeError):
                release.trusted_branch()
            git.assert_not_called()

    def test_tag_cannot_be_adopted_from_another_run(self):
        with patch.dict(os.environ, {"GITHUB_RUN_ID": "123"}), \
                patch.object(release, "git", side_effect=["tag", "Source-Commit: abc\nRelease-Run: 122"]), \
                self.assertRaises(RuntimeError):
            release.same_release("v0.1.0", "abc")

    def test_plan_rejects_branch_advancement_and_published_version(self):
        with patch.object(release, "trusted_branch", return_value="main"), \
                patch.object(release, "package_version", return_value="0.1.0"), \
                patch.object(release, "tag_exists", return_value=False), \
                patch.object(release, "git", side_effect=["abc", "def"]), \
                patch.object(release, "registry_version") as registry, self.assertRaises(RuntimeError):
            release.plan("patch")
        registry.assert_not_called()
        with patch.object(release, "trusted_branch", return_value="main"), \
                patch.object(release, "package_version", return_value="0.1.0"), \
                patch.object(release, "tag_exists", return_value=False), \
                patch.object(release, "git", return_value="abc"), \
                patch.object(release, "registry_version", return_value={"num": "0.1.1"}), \
                self.assertRaises(RuntimeError):
            release.plan("patch")

    def test_plan_emits_candidate_without_publish_token(self):
        with patch.dict(os.environ, {}, clear=True), \
                patch.object(release, "trusted_branch", return_value="main"), \
                patch.object(release, "package_version", return_value="0.1.0"), \
                patch.object(release, "tag_exists", return_value=False), \
                patch.object(release, "git", return_value="abc"), \
                patch.object(release, "registry_version", return_value=None), \
                contextlib.redirect_stdout(io.StringIO()) as output:
            release.plan("minor")
        self.assertIn('"version": "0.2.0"', output.getvalue())

    def test_manifest_push_publishes_only_unpublished_version(self):
        for existing, expected in ((None, "true"), ({"num": "0.1.0"}, "false")):
            with self.subTest(existing=existing), \
                    patch.dict(os.environ, {"GITHUB_EVENT_NAME": "push"}, clear=True), \
                    patch.object(release, "trusted_branch", return_value="main"), \
                    patch.object(release, "package_version", return_value="0.1.0"), \
                    patch.object(release, "tag_exists", return_value=False), \
                    patch.object(release, "git", return_value="abc"), \
                    patch.object(release, "registry_version", return_value=existing), \
                    contextlib.redirect_stdout(io.StringIO()) as output:
                release.plan("none")
            self.assertIn(f'"publish": "{expected}"', output.getvalue())
            self.assertIn('"version": "0.1.0"', output.getvalue())

    def test_published_automatic_release_can_resume_only_its_own_run(self):
        for same_run in (False, True):
            with self.subTest(same_run=same_run), \
                    patch.dict(os.environ, {"GITHUB_EVENT_NAME": "push"}, clear=True), \
                    patch.object(release, "trusted_branch", return_value="main"), \
                    patch.object(release, "package_version", return_value="0.1.0"), \
                    patch.object(release, "tag_exists", return_value=True), \
                    patch.object(release, "tagged_by_this_run", return_value=same_run), \
                    patch.object(release, "git", return_value="abc"), \
                    patch.object(release, "registry_version", return_value={"num": "0.1.0"}), \
                    contextlib.redirect_stdout(io.StringIO()) as output:
                release.plan("none")
            self.assertIn(f'"publish": "{str(same_run).lower()}"', output.getvalue())

    def test_manifest_push_cannot_apply_implicit_bump(self):
        with patch.dict(os.environ, {"GITHUB_EVENT_NAME": "push"}, clear=True), \
                patch.object(release, "trusted_branch", return_value="main"), \
                self.assertRaises(RuntimeError):
            release.plan("patch")

    def test_missing_publish_token_fails_before_any_mutation(self):
        with patch.dict(os.environ, {}, clear=True), patch.object(release, "git") as git, \
                patch.object(release, "set_version") as set_version, self.assertRaises(RuntimeError):
            release.publish("a" * 40, "0.1.0")
        git.assert_not_called()
        set_version.assert_not_called()


class RegistryTests(unittest.TestCase):
    def test_identity_check_rejects_different_or_yanked_package(self):
        for data in ({"checksum": "other", "yanked": False}, {"checksum": "same", "yanked": True}):
            with self.subTest(data=data), patch.object(release, "registry_version", return_value=data), \
                    self.assertRaises(RuntimeError):
                release.verify_registry("0.1.0", "same")

    def test_upload_polling_verifies_checksum(self):
        with patch.object(release, "registry_version", side_effect=[None, {"checksum": "same"}]), \
                patch.object(release.time, "sleep") as sleep:
            self.assertTrue(release.verify_registry("0.1.0", "same", attempts=2))
        sleep.assert_called_once_with(5)

    def test_missing_version_is_unpublished(self):
        with patch.object(release, "registry_version", return_value=None):
            self.assertFalse(release.verify_registry("0.1.0", "same"))


if __name__ == "__main__":
    unittest.main()
