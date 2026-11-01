static u64 be64(const u8 *p) { return (u64)be32(p) << 32 | be32(p + 4); }
static int native_plan(const u8 *data, unsigned size, char *args) {
  static const char magic[] = "BOOTAH-OBP/1\nloader=";
  if (size > 1024 || size < sizeof(magic) || !equal(data, magic, sizeof(magic) - 1)) return 0;
  unsigned at = sizeof(magic) - 1, name = 0, count = 0;
  while (at < size && data[at] != '\n') {
    if (name == 127 || !data[at]) return 0;
    bootfile[name++] = data[at++];
  }
  bootfile[name] = 0;
  if (!safe_bootfile(bootfile) || (name == 14 && equal(bootfile, "bootah-obp.cfg", 14))) return 0;
  if (at == size || size - ++at < 9 || !equal(data + at, "bootargs=", 9)) return 0;
  at += 9;
  while (at < size && data[at] != '\n') {
    if (count == 511 || data[at] < 32 || data[at] == 127) return 0;
    args[count++] = data[at++];
  }
  args[count] = 0;
  return count && at + 1 == size && data[at] == '\n';
}
static u64 native_image(const u8 *data, u64 size) {
  if (size < 64 || !equal(data, "\177ELF\2\2\1", 7) || be16(data + 16) != 2 ||
      be16(data + 18) != 43 || be32(data + 20) != 1 || be16(data + 52) != 64 ||
      be16(data + 54) != 56) return 0;
  u64 table = be64(data + 32), entry = be64(data + 24);
  unsigned count = be16(data + 56), executable = 0, loads = 0;
  if (!count || count > 16 || table > size || count * 56UL > size - table || (entry & 3)) return 0;
  for (unsigned i = 0; i < count; i++) {
    const u8 *p = data + table + i * 56;
    if (be32(p) != 1) continue;
    u64 offset = be64(p + 8), address = be64(p + 16), file = be64(p + 32), memory = be64(p + 40);
    if (!memory || file > memory || offset > size || file > size - offset ||
        address < 0x4000 || address >= 0x500000 || memory > 0x500000 - address ||
        be64(p + 24) != address) return 0;
    for (unsigned j = 0; j < i; j++) {
      const u8 *q = data + table + j * 56;
      if (be32(q) == 1 && address < be64(q + 16) + be64(q + 40) && be64(q + 16) < address + memory) return 0;
    }
    if ((be32(p + 4) & 1) && entry >= address && entry < address + file) executable = 1;
    loads++;
  }
  return loads && executable ? entry : 0;
}
