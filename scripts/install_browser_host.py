#!/usr/bin/env python3
"""Register Burrata's native host. --output-dir writes portable inspection fixtures."""
import argparse
import base64
import hashlib
import json
import os
from pathlib import Path
import shlex
import sys

HOST = 'org.burrata.browser'
FIREFOX_ID = 'burrata-browser@burrata.local'

def chromium_id(manifest):
    key = json.loads(manifest.read_text())['key']
    digest = hashlib.sha256(base64.b64decode(key, validate=True)).hexdigest()[:32]
    return ''.join(chr(ord('a') + int(c, 16)) for c in digest)

def host_paths(browser, home, platform):
    if platform == 'darwin':
        prefix = home / 'Library/Application Support'
        return {'firefox':prefix/'Mozilla/NativeMessagingHosts','chrome':prefix/'Google/Chrome/NativeMessagingHosts','chromium':prefix/'Chromium/NativeMessagingHosts'}[browser]
    if platform == 'win32':
        return Path(os.environ.get('LOCALAPPDATA', home/'AppData/Local'))/'Burrata/NativeMessagingHosts'/browser
    return {'firefox':home/'.mozilla/native-messaging-hosts','chrome':home/'.config/google-chrome/NativeMessagingHosts','chromium':home/'.config/chromium/NativeMessagingHosts'}[browser]

def install(args):
    executable = Path(args.executable).expanduser().resolve()
    if not executable.is_file():
        raise SystemExit(f'Executable not found: {executable}')
    if sys.platform != 'win32' and not os.access(executable, os.X_OK):
        raise SystemExit(f'Executable is not executable: {executable}')
    extension_id = args.chromium_extension_id or chromium_id(Path(__file__).resolve().parents[1]/'extension/manifest.chromium.json')
    if len(extension_id) != 32 or any(c not in 'abcdefghijklmnop' for c in extension_id):
        raise SystemExit('Chromium extension ID must contain 32 letters a–p.')
    browsers = ['firefox','chrome','chromium'] if args.browser == 'all' else [args.browser]
    for browser in browsers:
        directory = Path(args.output_dir).resolve()/browser if args.output_dir else host_paths(browser, Path.home(), sys.platform)
        directory.mkdir(parents=True, exist_ok=True)
        wrapper = directory/('burrata-native-host.cmd' if sys.platform=='win32' else 'burrata-native-host')
        if sys.platform == 'win32':
            # Native hosts on Windows officially require an executable, not cmd scripts.
            # Generate a small executable launcher using the installed Rust compiler.
            import subprocess
            wrapper = directory/'burrata-native-host.exe'
            source = directory/'launcher.rs'
            # A JSON string is also a valid Rust string literal for ordinary Windows paths.
            escaped = json.dumps(str(executable),ensure_ascii=False)
            source.write_text('fn main() { let status = std::process::Command::new('+escaped+').arg("--native-messaging").status().expect("Cannot start Burrata"); std::process::exit(status.code().unwrap_or(1)); }\n')
            subprocess.run(['rustc',str(source),'-o',str(wrapper)],check=True)
            source.unlink()
        else:
            wrapper.write_text('#!/bin/sh\nexec '+shlex.quote(str(executable))+' --native-messaging\n')
            wrapper.chmod(0o700)
        manifest = {'name':HOST,'description':'Burrata local browser import bridge','path':str(wrapper),'type':'stdio'}
        if browser == 'firefox':
            manifest['allowed_extensions']=[FIREFOX_ID]
        else:
            manifest['allowed_origins']=[f'chrome-extension://{extension_id}/']
        target = directory/(HOST+'.json')
        target.write_text(json.dumps(manifest,indent=2)+'\n')
        if sys.platform == 'win32' and not args.output_dir:
            import winreg
            registry = 'Mozilla' if browser=='firefox' else 'Google\\Chrome' if browser=='chrome' else 'Chromium'
            with winreg.CreateKey(winreg.HKEY_CURRENT_USER,rf'Software\{registry}\NativeMessagingHosts\{HOST}') as key:
                winreg.SetValueEx(key,'',0,winreg.REG_SZ,str(target))
        print(f'{browser}: {target}')
    print(f'Chromium extension ID: {extension_id}')

def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--executable',required=True,help='Built Burrata desktop executable')
    parser.add_argument('--browser',choices=['firefox','chrome','chromium','all'],default='firefox')
    parser.add_argument('--output-dir',help='Write manifests and launchers here instead of registering them')
    parser.add_argument('--chromium-extension-id',help='Override extension ID (defaults to committed manifest key)')
    install(parser.parse_args())

if __name__=='__main__':
    main()
