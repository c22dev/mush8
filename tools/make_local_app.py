#!/usr/bin/env python3
"""Package an already-built runtime with private local paths and native settings."""
import argparse
import json
import plistlib
import shutil
import subprocess
from pathlib import Path


def main():
    root = Path(__file__).resolve().parents[1]
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--game-dir', type=Path, required=True, help='prod.keys, base.nsp, upd4-0-1.nsp, optional dlc.nsp/Firmware')
    parser.add_argument('--base-nca', type=Path)
    parser.add_argument('--patched', type=Path, default=root / 'work/v-upd/patched')
    parser.add_argument('--npdm', type=Path, default=root / 'work/v-upd/exefs/main.npdm')
    parser.add_argument('--runtime', type=Path, default=root / 'target/release/nxhost')
    parser.add_argument('--font', type=Path, default=root / 'work/host-font.ttf')
    parser.add_argument('--network', choices=['off', 'lan', 'lanplay', 'wireless'], default='off')
    parser.add_argument('--server', default='', help='LAN-Play server host[:port] for --network lanplay')
    parser.add_argument('--language', default='system')
    parser.add_argument('--output', type=Path, default=root / 'build/MK8 Native.app')
    args = parser.parse_args()
    if args.network == 'lanplay' and not args.server.strip():
        parser.error('--network lanplay requires --server host[:port]')
    game = args.game_dir.resolve()
    runtime, patched, npdm, font = [p.resolve() for p in [args.runtime, args.patched, args.npdm, args.font]]
    base = args.base_nca.resolve() if args.base_nca else game / 'base.nsp'
    required = [runtime, npdm, game / 'prod.keys', base, game / 'upd4-0-1.nsp']
    required += [patched / f'{name}.{ext}' for name in ['main', 'subsdk0', 'sdk'] for ext in ['bin', 'layout']]
    missing = [str(p) for p in required if not p.is_file()]
    if missing:
        parser.error('Missing files: ' + ', '.join(missing))
    if not font.is_file() and not (game / 'Firmware').is_dir():
        parser.error('Provide Firmware or prepare a local --font (see ADAPTATIONS.md)')
    state = root / 'work/local-app'
    state.mkdir(parents=True, exist_ok=True)
    command = ['--patched', str(patched), '--npdm', str(npdm), '--keys', str(game / 'prod.keys'),
               '--base-nca' if args.base_nca else '--base', str(base), '--update', str(game / 'upd4-0-1.nsp'),
               '--save', str(state / 'save'), '--sdmc', str(state / 'sdmc'),
               '--system', str(game / 'system'), '--firmware', str(game / 'Firmware')]
    if (game / 'dlc.nsp').is_file():
        command += ['--dlc', str(game / 'dlc.nsp')]
    env = {'NX_WINDOWED': '1', 'NX_PHONE_PADS': '1', 'NX_NET': args.network,
           'NX_SHADER_CACHE': str(state / 'shader-cache')}
    if args.network == 'lanplay':
        env['NX_LANPLAY_SERVER'] = args.server.strip()
    if font.is_file():
        env['NX_FALLBACK_FONT'] = str(font)
    if args.language != 'system':
        env['NX_LANGUAGE'] = args.language
    app = args.output.resolve()
    mac, resources = app / 'Contents/MacOS', app / 'Contents/Resources'
    mac.mkdir(parents=True, exist_ok=True)
    resources.mkdir(parents=True, exist_ok=True)
    shutil.copy2(runtime, mac / 'nxhost')
    info = {'CFBundleName': 'MK8 Native', 'CFBundleIdentifier': 'org.mush8.local',
            'CFBundleExecutable': 'nxhost', 'CFBundlePackageType': 'APPL', 'CFBundleVersion': '1',
            'NSHighResolutionCapable': True,
            'NSLocalNetworkUsageDescription': 'Find nearby multiplayer clients and iPhone controllers.',
            'NSBonjourServices': ['_mush8._tcp', '_mush8-pad._udp', '_mush8-mp._udp']}
    (app / 'Contents/Info.plist').write_bytes(plistlib.dumps(info))
    (resources / 'launch.json').write_text(json.dumps({'cwd': str(state), 'args': command, 'env': env,
                                                     'log': str(state / 'nxhost.log')}, indent=2) + '\n')
    subprocess.run(['codesign', '--force', '--deep', '--sign', '-', str(app)], check=True)
    # Signing alone does not catch missing Swift @rpath dependencies.
    subprocess.run([str(mac / 'nxhost'), '--help'], check=True, stdout=subprocess.DEVNULL)
    print(app)


if __name__ == '__main__':
    main()
