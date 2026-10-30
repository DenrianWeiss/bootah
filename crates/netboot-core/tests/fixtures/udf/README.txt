Generated with tools/udf-fixtures using libmkiso 0.1.5.
These synthetic files contain only Bootah test data, not operating system media.

Reproduce from the repository root:
  cargo run --manifest-path tools/udf-fixtures/Cargo.toml -- .artifacts/udf-fixtures

Profiles: VAT 1.50, VAT 2.01, sparable with a relocated packet,
UDF 2.50 mirrored metadata, UDF 2.60 mirrored metadata over sparable mapping.
Each includes a long ASCII filename under a Chinese directory, a Chinese file,
an empty file, and a 20,003-byte payload where byte n equals n modulo 251.
Payload SHA-256:
  ee00de5b1eb73f0d3b7b22862029ded78a3f153910ce1b6f2e4862c04773adf8

All five images were independently mounted read-only by Linux in isolated QEMU.
Reader references: ECMA-167 and OSTA UDF 2.60.
