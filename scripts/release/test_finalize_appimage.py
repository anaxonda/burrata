"""AppImage relocation must preserve helper hashes and the host environment."""
import importlib.util
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest.mock import patch

SPEC = importlib.util.spec_from_file_location('finalize', Path(__file__).with_name('finalize-appimage.py'))
finalize = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(finalize)


class RelocationTests(unittest.TestCase):
    def test_nested_elf_gets_bundle_path_but_pinned_helpers_and_links_are_untouched(self):
        with tempfile.TemporaryDirectory() as directory:
            app = Path(directory).resolve()
            helper = app / 'usr/lib/webkit/process'
            pinned = app / 'usr/lib/Burrata/resources/tools/ffmpeg'
            for p in (helper, pinned):
                p.parent.mkdir(parents=True, exist_ok=True)
                p.write_bytes(b'\x7fELFfixture')
            try:
                (helper.parent / 'alias').symlink_to(helper)
            except OSError:
                pass
            original = pinned.read_bytes()
            with patch.object(finalize.subprocess, 'run', return_value=subprocess.CompletedProcess([], 0, '$ORIGIN:/custom\n', '')) as run:
                self.assertEqual(finalize.relocate_runtime(app, Path('/patchelf')), 1)
            self.assertEqual(run.call_args_list[1].args[0], [str(Path('/patchelf')), '--set-rpath', '$ORIGIN/..:$ORIGIN:/custom', str(helper)])
            self.assertEqual(len(run.call_args_list), 2)
            self.assertEqual(pinned.read_bytes(), original)

    def test_repacking_preserves_the_selected_runtime_and_validates_its_boundary(self):
        with tempfile.TemporaryDirectory() as directory:
            installer = Path(directory) / 'fixture.AppImage'
            destination = Path(directory) / 'runtime'
            prefix = b'\x7fELF' + bytes(4092)
            installer.write_bytes(prefix + b'hsqs' + bytes(64))
            with patch.object(finalize.subprocess, 'check_output', return_value='4096\n'):
                finalize.preserve_runtime(installer, destination)
            self.assertEqual(destination.read_bytes(), prefix)
            installer.write_bytes(prefix + b'bad!')
            with patch.object(finalize.subprocess, 'check_output', return_value='4096\n'):
                with self.assertRaisesRegex(ValueError, 'signature'):
                    finalize.preserve_runtime(installer, destination)

    def test_launcher_preserves_host_libraries_and_arguments(self):
        if finalize.os.name == 'nt':
            self.skipTest('Linux launcher')
        with tempfile.TemporaryDirectory() as directory:
            app = Path(directory).resolve()
            (app / 'usr/bin').mkdir(parents=True)
            (app / 'apprun-hooks').mkdir()
            for hook in ('gtk', 'gstreamer'):
                (app / f'apprun-hooks/linuxdeploy-plugin-{hook}.sh').write_text(':\n')
            binary = app / 'usr/bin/burrata'
            binary.write_text('#!/bin/bash\nprintf "%s\\n" "$LD_LIBRARY_PATH" "$PWD" "$APPDIR" "$@"\n')
            binary.chmod(0o755)
            (app / 'AppRun.wrapped').write_text('legacy')
            finalize.install_launcher(app)
            env = {**finalize.os.environ, 'LD_LIBRARY_PATH': '/host/library'}
            env.pop('APPDIR', None)
            result = subprocess.check_output([str(app / 'AppRun'), 'space value', '--flag'], env=env, text=True)
            self.assertEqual(result.splitlines(), ['/host/library', str(app / 'usr'), str(app), 'space value', '--flag'])
            self.assertFalse((app / 'AppRun.wrapped').exists())


if __name__ == '__main__':
    unittest.main()
