#define GRUB_ADDRESS 0x4400UL
#define GRUB_RESERVATION 0x800000UL
#define GRUB_FILE_LIMIT 0x200000UL
static int grub_image(const u8 *p, u64 size) {
  if (size < 64 || size > GRUB_FILE_LIMIT || be32(p) != 0x01030107 ||
      be32(p + 4) != size - 32 || be32(p + 8) || be32(p + 12) ||
      be32(p + 16) || be32(p + 20) != GRUB_ADDRESS ||
      be32(p + 24) || be32(p + 28))
    return 0;
  /* SPARC64 GRUB crt0 branches over its module-size field, preserving CIF. */
  return (be32(p + 32) & 0xffc00000) == 0x10800000 &&
         be32(p + 36) == 0x9010000c;
}
static int safe_bootfile(const char *s) {
  unsigned n = 0;
  while (s[n]) {
    unsigned c = (u8)s[n++];
    if (n > 127 || c <= 32 || c >= 127 || c == ':' || c == ',' || c == '\\')
      return 0;
  }
  if (!n) return 0;
  const char *base = s;
  for (unsigned i = 0; i < n; i++) {
    if (s[i] == '/' && (i == 0 || s[i - 1] == '/')) return 0;
    if (s[i] == '.' && (i == 0 || s[i - 1] == '/') &&
        (!s[i + 1] || s[i + 1] == '/' || s[i + 1] == '.')) return 0;
    if (s[i] == '/') base = s + i + 1;
  }
  if (!*base || (length(base) == 14 && equal(base, "bootah-obp.bin", 14))) return 0;
  unsigned hexchars = 0;
  while ((base[hexchars] >= '0' && base[hexchars] <= '9') ||
         (base[hexchars] >= 'A' && base[hexchars] <= 'F') ||
         (base[hexchars] >= 'a' && base[hexchars] <= 'f')) hexchars++;
  return !(hexchars == 8 && (!base[8] || base[8] == '.'));
}
static unsigned handoff_packet(u8 *p) {
  memset(p, 0, 512);
  p[0] = 2; p[1] = 1; p[2] = 6;
  put32(p + 4, xid);
  put32(p + 16, ip); put32(p + 20, server);
  memcpy(p + 28, mac, 6);
  memcpy(p + 108, bootfile, length(bootfile) + 1);
  put32(p + 236, 0x63825363);
  unsigned at = 240;
  unsigned tags[] = {53, 1, 3, 51, 54};
  u32 values[] = {5, netmask, gateway, lease_seconds, address_server};
  for (unsigned i = 0; i < 5; i++) {
    p[at++] = tags[i]; p[at++] = i ? 4 : 1;
    if (i) { put32(p + at, values[i]); at += 4; }
    else p[at++] = 5;
  }
  p[at++] = 67; p[at++] = length(bootfile);
  memcpy(p + at, bootfile, length(bootfile)); at += length(bootfile);
  p[at++] = 255;
  return at;
}
static int path_text(char **out, unsigned *left, const char *text) {
  while (*text) {
    if (*left < 2) return 0;
    *(*out)++ = *text++; --*left;
  }
  **out = 0; return 1;
}
static int path_ip(char **out, unsigned *left, u32 address) {
  char text[16], *p = text;
  for (int i = 3; i >= 0; i--) {
    unsigned value = (address >> (8 * i)) & 255;
    if (value >= 100) *p++ = '0' + value / 100;
    if (value >= 10) *p++ = '0' + value / 10 % 10;
    *p++ = '0' + value % 10;
    if (i) *p++ = '.';
  }
  *p = 0; return path_text(out, left, text);
}
static int handoff_path(char *out, unsigned capacity, const char *device, int native) {
  if (!capacity) return 0;
  if (!path_text(&out, &capacity, device) || !path_text(&out, &capacity, ":") ||
      !path_ip(&out, &capacity, server) || !path_text(&out, &capacity, ",") ||
      !path_text(&out, &capacity, bootfile)) return 0;
  if (!native) return 1;
  return path_text(&out, &capacity, ",") && path_ip(&out, &capacity, ip) &&
      path_text(&out, &capacity, ",") && path_ip(&out, &capacity, gateway) &&
      path_text(&out, &capacity, ",5,5,") && path_ip(&out, &capacity, netmask) &&
      path_text(&out, &capacity, ",512");
}
