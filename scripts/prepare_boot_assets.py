#!/usr/bin/env python3
import argparse,concurrent.futures,hashlib,json,pathlib,shutil,subprocess,urllib.request
ROOT=pathlib.Path(__file__).resolve().parents[1]
DEST=ROOT/'.artifacts/boot-assets'
URLS={'wimboot-arm64':'https://github.com/ipxe/wimboot/releases/download/v2.9.0/wimboot.arm64','undionly.kpxe':'https://boot.ipxe.org/undionly.kpxe','wimboot':'https://github.com/ipxe/wimboot/releases/download/v2.9.0/wimboot'}
for name,directory in [('x64','x86_64-efi'),('ia32','i386-efi'),('arm32','arm32-efi'),('arm64','arm64-efi'),('riscv64','riscv64-efi'),('loongarch64','loong64-efi')]:URLS[f'ipxe-{name}.efi']=f'https://boot.ipxe.org/{directory}/ipxe.efi'
URLS['snponly-x64.efi']='https://boot.ipxe.org/x86_64-efi/snponly.efi'
def fetch(item):
 name,url=item;path=DEST/name
 if not path.exists():
  with urllib.request.urlopen(url,timeout=60) as response:data=response.read()
  if len(data)<10000:raise ValueError(f'Invalid boot asset: {name}')
  tmp=path.with_suffix('.tmp');tmp.write_bytes(data);tmp.replace(path)
 return name,{'url':url,'sha256':hashlib.sha256(path.read_bytes()).hexdigest()}
if __name__=='__main__':
 parser=argparse.ArgumentParser();parser.add_argument('--linux-media',action='store_true',help='Build optional static x86-64 Linux ISO compatibility helper');args=parser.parse_args()
 DEST.mkdir(parents=True,exist_ok=True)
 with concurrent.futures.ThreadPoolExecutor(max_workers=4) as pool:manifest=dict(pool.map(fetch,URLS.items()))
 (DEST/'sources.json').write_text(json.dumps(manifest,indent=2))
 shutil.copy2(ROOT/'LICENSES/boot-assets.md',DEST/'LICENSES.md')
 if args.linux_media:subprocess.run(['sh',str(ROOT/'tools/linux-media/build.sh')],cwd=ROOT,check=True)
 print(DEST)
