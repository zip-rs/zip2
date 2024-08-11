#!/usr/bin/env python

import sys
from pathlib import Path
from zipfile import ZipFile, ZIP_STORED, ZIP_DEFLATED, ZIP_BZIP2

compressible_text_file = Path(__file__).parent / 'folder/king-lear.txt'

def log(msg):
  print(msg, file=sys.stderr)

with ZipFile('out.zip', mode='w') as zf:
  for i in range(50):
    log(f"i={i}")
    zf.write(compressible_text_file, arcname=f"stored-n{i}.txt",
             compress_type=ZIP_STORED)
    log('stored')
    zf.write(compressible_text_file, arcname=f"deflated-n{i}.txt",
             compress_type=ZIP_DEFLATED, compresslevel=9)
    log('deflated')
    zf.write(compressible_text_file, arcname=f"bzip2-n{i}.txt",
             compress_type=ZIP_BZIP2, compresslevel=9)
    log('bzip2')
