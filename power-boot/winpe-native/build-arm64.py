#!/usr/bin/env python3
"""Build the ARM64 WinPE helper using Clang, LLD and permissive MinGW headers."""
import argparse
import pathlib
import shutil
import subprocess
import tempfile

p = argparse.ArgumentParser(description=__doc__)
p.add_argument('--headers', type=pathlib.Path, required=True)
p.add_argument('--clang', default='clang')
p.add_argument('--lld', required=True, help='lld-link, or rust-lld with the COFF flavor')
a = p.parse_args()
root = pathlib.Path(__file__).resolve().parents[2]
output = root/'.artifacts/boot-assets/bootah-native-arm64.exe'
output.parent.mkdir(parents=True, exist_ok=True)
imports = {
    'kernel32': 'WriteFile GetStdHandle lstrlenA GetCurrentProcess GetFirmwareEnvironmentVariableW CloseHandle CreateFileW DeleteFileW GetCommandLineW LocalFree ExitProcess',
    'advapi32': 'OpenProcessToken LookupPrivilegeValueW AdjustTokenPrivileges',
    'shell32': 'CommandLineToArgvW',
    'winhttp': 'WinHttpCrackUrl WinHttpOpen WinHttpSetTimeouts WinHttpConnect WinHttpOpenRequest WinHttpSetOption WinHttpSendRequest WinHttpReceiveResponse WinHttpQueryHeaders WinHttpReadData WinHttpCloseHandle',
}
lld = [a.lld] + (['-flavor', 'link'] if pathlib.Path(a.lld).name == 'rust-lld' else [])
with tempfile.TemporaryDirectory(prefix='bootah-arm64-') as temporary:
    work = pathlib.Path(temporary)
    obj = work/'bootstrap.obj'
    subprocess.run([a.clang, '--target=aarch64-w64-windows-gnu', '-DBOOTAH_NO_CRT', '-Os',
                    '-fno-stack-protector', '-ffreestanding', '-Wno-pragma-pack', '-isystem', str(a.headers),
                    '-c', str(root/'tools/winpe-native/bootstrap.c'), '-o', str(obj)], check=True)
    libraries = []
    for library, symbols in imports.items():
        definition = work/f'{library}.def'
        definition.write_text(f'LIBRARY {library}.dll\nEXPORTS\n' + '\n'.join(symbols.split()) + '\n')
        lib = work/f'{library}.lib'
        subprocess.run(lld + ['/lib', '/machine:arm64', f'/def:{definition}', f'/out:{lib}'], check=True)
        libraries.append(str(lib))
    binary = work/'bootah-native-arm64.exe'
    subprocess.run(lld + ['/machine:arm64', '/subsystem:console', '/entry:mainCRTStartup', '/nodefaultlib',
                         '/dynamicbase', '/nxcompat', f'/out:{binary}', str(obj)] + libraries, check=True)
    shutil.copyfile(binary, output)
print(output)
