#!/usr/bin/env python3
"""Sun a.out OMAGIC network executable, with initialized working memory."""
import pathlib
import struct
import sys
image = pathlib.Path(sys.argv[1]).read_bytes()
pathlib.Path(sys.argv[2]).write_bytes(struct.pack('>8I', 0x01030107, len(image), 0, 0, 0, 0x4000, 0, 0) + image)
