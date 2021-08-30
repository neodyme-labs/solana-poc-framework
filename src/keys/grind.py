import subprocess
import os

cmd = ['solana-keygen', 'grind']
for i in range(256):
    cmd.append('--starts-with')
    cmd.append('K' + str(i).replace('0', 'o').rjust(3, 'o') + ':1')
subprocess.run(cmd)

keyfiles = [file for file in os.listdir('.') if file.endswith('.json')]
keyfiles.sort(key=lambda f: int(f[1:4].replace('o', '0')))

print('pub const KEYPAIRS: [[u8; 64]; 256] = [')
for file in keyfiles:
    print(f'    include!("keys/{file}"),')
print('];')
