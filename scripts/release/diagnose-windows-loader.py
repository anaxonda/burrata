#!/usr/bin/env python3
"""Failure-only PE import/export audit. Requires Windows and `python -m pip install pefile`.
Does not launch the failed executable or initialize imported DLLs.
"""
import ctypes
from ctypes import wintypes
import json
import os
from pathlib import Path
import sys
import pefile

if os.name != 'nt':
    raise SystemExit('Run on the failing Windows runner')
exe = Path(sys.argv[1]).resolve()
kernel = ctypes.WinDLL('kernel32', use_last_error=True)
kernel.LoadLibraryExW.argtypes = [wintypes.LPCWSTR, wintypes.HANDLE, wintypes.DWORD]
kernel.LoadLibraryExW.restype = wintypes.HMODULE
kernel.GetModuleFileNameW.argtypes = [wintypes.HMODULE, wintypes.LPWSTR, wintypes.DWORD]
kernel.GetModuleFileNameW.restype = wintypes.DWORD
system_root = Path(os.environ.get('SystemRoot', r'C:\Windows'))
search = [exe.parent, system_root / 'System32', system_root, Path.cwd()]
search += [Path(p) for p in os.environ.get('PATH', '').split(';') if p]
report = {'executable':str(exe), 'path':os.environ.get('PATH'), 'modules':[], 'missing':[]}
queue = [exe]
seen = set()
pe_cache = {}

def load_pe(path):
    key = str(path).lower()
    if key not in pe_cache:
        parsed = pefile.PE(str(path), fast_load=True)
        parsed.parse_data_directories(directories=[
            pefile.DIRECTORY_ENTRY['IMAGE_DIRECTORY_ENTRY_IMPORT'],
            pefile.DIRECTORY_ENTRY['IMAGE_DIRECTORY_ENTRY_DELAY_IMPORT'],
            pefile.DIRECTORY_ENTRY['IMAGE_DIRECTORY_ENTRY_EXPORT'],
        ])
        pe_cache[key] = parsed
    return pe_cache[key]

def resolve(name):
    candidates = []
    for directory in search:
        candidate = directory / name
        if candidate.is_file() and str(candidate).lower() not in [str(p).lower() for p in candidates]:
            candidates.append(candidate.resolve())
    # API-set contracts may have no disk file; the loader maps them to host DLLs.
    if not candidates and name.lower().startswith(('api-ms-', 'ext-ms-')):
        handle = kernel.LoadLibraryExW(name, None, 1)  # DONT_RESOLVE_DLL_REFERENCES
        if handle:
            buffer = ctypes.create_unicode_buffer(32768)
            if kernel.GetModuleFileNameW(handle, buffer, len(buffer)):
                candidates.append(Path(buffer.value))
    return candidates

while queue and len(seen) < 300:
    module = queue.pop(0)
    key = str(module).lower()
    if key in seen: continue
    seen.add(key)
    print('Inspecting ' + str(module), file=sys.stderr, flush=True)
    try:
        pe = load_pe(module)
    except Exception as error:
        report['modules'].append({'path':str(module), 'error':str(error)})
        continue
    imports = []
    for table in ('DIRECTORY_ENTRY_IMPORT', 'DIRECTORY_ENTRY_DELAY_IMPORT'):
        for descriptor in getattr(pe, table, []):
            name = descriptor.dll.decode(errors='replace')
            candidates = resolve(name)
            row = {'dll':name, 'delay':table.endswith('DELAY_IMPORT'), 'candidates':[str(p) for p in candidates]}
            if not candidates:
                row['unresolved'] = True
                imports.append(row)
                continue
            selected = candidates[0]
            target = load_pe(selected)
            exports = getattr(target, 'DIRECTORY_ENTRY_EXPORT', None)
            symbols = exports.symbols if exports else []
            names = {symbol.name for symbol in symbols}
            ordinals = {symbol.ordinal for symbol in symbols}
            missing = [imp.name.decode(errors='replace') if imp.name else '#'+str(imp.ordinal)
                       for imp in descriptor.imports
                       if (imp.name not in names if imp.name else imp.ordinal not in ordinals)]
            row['missing_exports'] = missing
            if missing:
                report['missing'].append({'importer':str(module), 'dll':str(selected), 'symbols':missing, 'delay':row['delay']})
            imports.append(row)
            queue.append(selected)
    report['modules'].append({'path':str(module), 'imports':imports})
print(json.dumps(report, indent=2))
