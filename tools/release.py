#!/usr/bin/env python3
"""Bundle a verified device package and its committed Mac/source tree."""
import argparse
import hashlib
import io
import json
from pathlib import Path
import subprocess
import tarfile
import zipfile


def digest(data):
    return hashlib.sha256(data).hexdigest()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--package', required=True, type=Path)
    parser.add_argument('--version', required=True)
    parser.add_argument('--out', required=True, type=Path)
    args = parser.parse_args()
    root = Path(__file__).resolve().parents[1]
    commit = subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=root, text=True).strip()
    dirty = subprocess.check_output(['git', 'status', '--porcelain', '--untracked-files=no'],
                                    cwd=root, text=True)
    if dirty:
        raise SystemExit('Commit the corresponding source before packaging.')
    package = args.package.resolve()
    manifest = json.loads((package/'manifest.json').read_text())
    if manifest['source_commit'] != commit:
        raise SystemExit('Device package was built from a different source commit.')
    checksums = {}
    for line in (package/'SHA256SUMS').read_text().splitlines():
        checksum, name = line.split('  ', 1)
        path = Path(name)
        if path.is_absolute() or '..' in path.parts:
            raise SystemExit('Unsafe package checksum path.')
        checksums[name] = checksum
    names = {p.relative_to(package).as_posix() for p in package.rglob('*') if p.is_file()}
    if names != set(checksums) | {'SHA256SUMS'}:
        raise SystemExit('Package files differ from the checksum index.')
    for name, checksum in checksums.items():
        path = package/name
        if path.is_symlink() or digest(path.read_bytes()) != checksum:
            raise SystemExit('Package checksum failed for '+name)
    archive = subprocess.check_output(['git', 'archive', '--format=tar', commit], cwd=root)
    args.out.mkdir(parents=True, exist_ok=True)
    filename = 'kindle-pw2-preview-'+args.version+'.zip'
    dest = args.out/filename
    if dest.exists():
        raise SystemExit('Refusing to overwrite an existing release archive.')
    with zipfile.ZipFile(dest, 'x', compression=zipfile.ZIP_DEFLATED) as output:
        for path in sorted(package.rglob('*')):
            if path.is_file():
                output.write(path, 'kindle-native-sync-package/'+path.relative_to(package).as_posix())
        with tarfile.open(fileobj=io.BytesIO(archive), mode='r:') as source:
            for member in source:
                if member.isfile():
                    info = zipfile.ZipInfo('mac-assistant/'+member.name)
                    info.create_system = 3
                    info.external_attr = (0o100000 | member.mode) << 16
                    info.compress_type = zipfile.ZIP_DEFLATED
                    output.writestr(info, source.extractfile(member).read())
        output.writestr('START_HERE.md',
            '# Kindle PW2 experimental preview\n\n'
            'Open mac-assistant/docs/QUICKSTART.md for the complete instructions.\n'
            'The Mac helper needs Python 3.11+, then ./setup.sh.\n'
            'The device package is for B0D4 / 5.12.2.1.1 with working KUAL.\n'
            'Read the validation limitations before enabling uploads.\n'
            'Corresponding source commit: '+commit+'\n')
    (args.out/(filename+'.sha256')).write_text(digest(dest.read_bytes())+'  '+filename+'\n')
    print(dest)


if __name__ == '__main__':
    main()
