#!/usr/bin/env python3
"""Check reviewed dependency notices without invoking Cargo."""

import contextlib
import hashlib
import importlib.util
import io
import json
import os
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch


SCRIPT = Path(__file__).with_name("third-party-notices.py")
SPEC = importlib.util.spec_from_file_location("third_party_notices", SCRIPT)
NOTICES = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(NOTICES)
SOURCE = "registry+https://github.com/rust-lang/crates.io-index"


class Output(io.StringIO):
    def reconfigure(self, **kwargs):
        pass


class ReviewedNoticesTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        crate = self.root / "crate"
        crate.mkdir()
        self.package = {
            "name": "example", "version": "1.2.3", "source": SOURCE,
            "license": "MIT OR Apache-2.0",
            "manifest_path": str(crate / "Cargo.toml"),
        }
        self.contents = [b"Complete MIT notice\nCopyright Example\n",
                         b"Complete Apache notice\nAll terms and appendix\n"]
        files = []
        for name, content in zip(("LICENSE-MIT", "LICENSE-APACHE"), self.contents):
            relative = Path("third-party-licenses") / "example" / name
            path = self.root / relative
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(content)
            files.append({"path": relative.as_posix(),
                          "sha256": hashlib.sha256(content).hexdigest()})
        self.entry = {key: self.package[key]
                      for key in ("name", "version", "source", "license")}
        self.entry.update(upstream_archive_sha256="a" * 64,
                          provenance={"review": "test fixture"}, files=files)
        self.manifest = self.root / "third-party-notices.json"
        self.write_manifest()

    def write_manifest(self):
        self.manifest.write_text(json.dumps({"schema_version": 1,
                                            "packages": [self.entry]}),
                                 encoding="utf-8")

    def generate(self, packages=None):
        out = Output()
        with patch.object(NOTICES, "REVIEWED_MANIFEST", self.manifest, create=True), \
                patch.object(NOTICES, "metadata", return_value={}), \
                patch.object(NOTICES, "linked", return_value=packages or [self.package]), \
                patch.object(NOTICES.sys, "argv", [str(SCRIPT), "--target", "test"]), \
                contextlib.redirect_stdout(out):
            try:
                NOTICES.main()
            except SystemExit:
                self.assertEqual(out.getvalue(), "")
                raise
        return out.getvalue()

    def test_exact_reviewed_package_includes_complete_notices(self):
        output = self.generate()
        for content in self.contents:
            self.assertIn(content.decode().strip(), output)
        self.assertIn("example 1.2.3", output)

    def test_unknown_identity_or_license_is_refused(self):
        for key, value in (("name", "unknown"), ("version", "1.2.4"),
                           ("source", "git+https://example.invalid/repo"),
                           ("source", None), ("license", "MIT")):
            with self.subTest(key=key, value=value):
                package = dict(self.package, **{key: value})
                with self.assertRaises(SystemExit):
                    self.generate([package])

    def test_missing_reviewed_file_is_refused(self):
        (self.root / self.entry["files"][1]["path"]).unlink()
        with self.assertRaises(SystemExit):
            self.generate()

    def test_changed_reviewed_file_is_refused(self):
        (self.root / self.entry["files"][0]["path"]).write_bytes(b"Changed")
        with self.assertRaises(SystemExit):
            self.generate()

    def test_empty_reviewed_file_is_refused_even_with_matching_hash(self):
        item = self.entry["files"][0]
        (self.root / item["path"]).write_bytes(b"")
        item["sha256"] = hashlib.sha256(b"").hexdigest()
        self.write_manifest()
        with self.assertRaises(SystemExit):
            self.generate()

    def test_shipped_notices_take_priority_without_reading_manifest(self):
        normal = Path(self.package["manifest_path"]).with_name("LICENSE")
        normal.write_text("Original shipped notice", encoding="utf-8")
        self.manifest.unlink()
        output = self.generate()
        self.assertIn("Original shipped notice", output)
        self.assertNotIn("Complete MIT notice", output)

    def test_unknown_package_prevents_partial_output(self):
        unknown = dict(self.package, name="unknown")
        with self.assertRaises(SystemExit):
            self.generate([self.package, unknown])

    def test_matching_non_registry_manifest_is_still_refused(self):
        self.entry["source"] = "git+https://example.invalid/repo"
        self.write_manifest()
        package = dict(self.package, source=self.entry["source"])
        with self.assertRaises(SystemExit):
            self.generate([package])

    def test_notice_outside_tools_is_refused(self):
        self.entry["files"][0]["path"] = "../LICENSE"
        self.write_manifest()
        with self.assertRaises(SystemExit):
            self.generate()

    def test_actual_reviewed_notices_from_another_working_directory(self):
        self.manifest = SCRIPT.with_name("third-party-notices.json")
        manifest = json.loads(self.manifest.read_text(encoding="utf-8"))
        packages = [dict(entry, manifest_path=self.package["manifest_path"])
                    for entry in manifest["packages"]]
        previous = Path.cwd()
        try:
            os.chdir(self.root)
            output = self.generate(packages)
        finally:
            os.chdir(previous)
        for entry in manifest["packages"]:
            self.assertIn(f"{entry['name']} {entry['version']}", output)
            for item in entry["files"]:
                content = (self.manifest.parent / item["path"]).read_text(
                    encoding="utf-8").strip()
                self.assertIn(content, output)


if __name__ == "__main__":
    unittest.main()
