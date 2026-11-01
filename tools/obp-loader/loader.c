#include <stddef.h>
#include <stdint.h>
typedef uint64_t u64;
typedef uint32_t u32;
typedef uint16_t u16;
typedef uint8_t u8;
#define ALIGNED __attribute__((aligned(8)))
static int (*cif)(void *);
static u64 chosen, output, channel, original_stack;
static u8 rx[2048] ALIGNED, tx[2048] ALIGNED, mac[6], peer_mac[6];
static u32 ip, server, netmask, gateway, xid, lease_seconds, address_server;
static u64 lease_started;
static u16 source_port = 20000, peer_port;
static char bootfile[241], device[512], bootpath[1024], bootargs[512];
void *memcpy(void *d, const void *s, size_t n) {
  u8 *p = d; const u8 *q = s; while (n--) *p++ = *q++; return d;
}
void *memset(void *d, int c, size_t n) {
  u8 *p = d; while (n--) *p++ = c; return d;
}
static size_t length(const char *s) { size_t n = 0; while (s[n]) n++; return n; }
static int equal(const void *a, const void *b, size_t n) {
  const u8 *p = a, *q = b; while (n--) if (*p++ != *q++) return 0; return 1;
}
static u16 be16(const u8 *p) { return (u16)p[0] * 256 + p[1]; }
static u32 be32(const u8 *p) { return (u32)be16(p) * 65536 + be16(p + 2); }
static void put16(u8 *p, u16 v) { p[0] = v >> 8; p[1] = v; }
static void put32(u8 *p, u32 v) { put16(p, v >> 16); put16(p + 2, v); }
static long firmware_io(const char *op, u64 handle, void *buffer, unsigned size) {
  u64 args[] = {(u64)op, 3, 1, handle, (u64)buffer, size, (u64)-1};
  return cif(args) ? -1 : (long)args[6];
}
static void print(const char *s) { firmware_io("write", output, (void *)s, length(s)); }
static void hex(u64 n) {
  char s[17]; for (int i = 15; i >= 0; i--) { s[i] = "0123456789ABCDEF"[n & 15]; n >>= 4; }
  s[16] = 0; print(s);
}
static void fail(const char *s) __attribute__((noreturn));
static void fail(const char *s) {
  print("Bootah OBP: "); print(s); print("\r\n");
  u64 args[] = {(u64)"exit", 0, 0}; cif(args);
  for (;;) __asm__ volatile("nop");
}
static long property(u64 node, const char *name, void *p, unsigned n) {
  u64 args[] = {(u64)"getprop", 4, 1, node, (u64)name, (u64)p, n, (u64)-1};
  return cif(args) ? -1 : (long)args[7];
}
static u64 find(const char *path) {
  u64 a[] = {(u64)"finddevice", 1, 1, (u64)path, (u64)-1};
  return cif(a) ? (u64)-1 : a[4];
}
static u64 handle(const char *name) {
  u8 v[4]; if (property(chosen, name, v, 4) != 4) fail("missing chosen handle");
  return be32(v);
}
static u64 now(void) {
  u64 a[] = {(u64)"milliseconds", 0, 1, 0};
  if (cif(a)) fail("firmware timer unavailable");
  return (u32)a[3];
}
static int expired(u64 start, unsigned seconds) { return (u32)((u32)now() - (u32)start) >= seconds * 1000U; }
static int property_matches(u64 node, const char *name, const void *p, unsigned n) {
  static u8 checked[1024];
  return n <= sizeof(checked) && property(node, name, checked, n) == (long)n && equal(checked, p, n);
}
static int forthprop(u64 node, const char *name, const void *p, unsigned n) {
  static int initialized;
  if (!initialized) {
    static const char definitions[] =
      ": bootah-obp-copy-property 2swap encode-bytes 2swap property ; "
      ": bootah-obp-set-property my-self >r 0 to my-self push-package "
      "['] bootah-obp-copy-property catch pop-package r> to my-self throw ;";
    u64 define[] = {(u64)"interpret", 1, 1, (u64)definitions, (u64)-1};
    if (cif(define) || define[4]) return 0;
    initialized = 1;
  }
  u64 args[] = {(u64)"interpret", 6, 1, (u64)"bootah-obp-set-property",
                node, length(name), (u64)name, n, (u64)p, (u64)-1};
  return !cif(args) && !args[9];
}
static void nodeprop(u64 node, const char *name, const void *p, unsigned n) {
  int bootpath_property = length(name) == 8 && equal(name, "bootpath", 8);
  int bootargs_property = length(name) == 8 && equal(name, "bootargs", 8);
  if (node && node != (u64)-1 && n <= 1024) {
    if (property_matches(node, name, p, n)) return;
    if ((bootpath_property && n > 256) || (bootargs_property && n > 128)) goto failed;
    if (bootpath_property || bootargs_property) {
      static unsigned initialized_strings;
      unsigned bit = bootpath_property ? 1 : 2;
      /* Sun chosen-string setters do not accept the generic encoded pair.
         Use the original buffer only when the property points to that buffer. */
      const char *definition = bootpath_property ?
        ": bootah-obp-path >r \" path-buf\" $find if execute \" bootpath\" r> get-package-property "
        "if drop 2drop 0 else drop over = if swap move -1 else drop 2drop 0 then then "
        "else 2drop r> drop 2drop 0 then ;" :
        ": bootah-obp-args >r \" args-buf\" $find if execute \" bootargs\" r> get-package-property "
        "if drop 2drop 0 else drop over = if swap move -1 else drop 2drop 0 then then "
        "else 2drop r> drop 2drop 0 then ;";
      if (!n || ((const u8 *)p)[n - 1]) goto failed;
      if (!(initialized_strings & bit)) {
        u64 define[] = {(u64)"interpret", 1, 1, (u64)definition, (u64)-1};
        if (cif(define) || define[4]) goto failed;
        initialized_strings |= bit;
      }
      u64 update[] = {(u64)"interpret", 4, 2,
        (u64)(bootpath_property ? "bootah-obp-path" : "bootah-obp-args"), node, n, (u64)p, (u64)-1, 0};
      if (!cif(update) && !update[7] && update[8]) {
        if (property_matches(node, name, p, n)) return;
        goto failed;
      }
    }
    u64 args[] = {(u64)"setprop", 4, 1, node, (u64)name, (u64)p, n, (u64)-1};
    cif(args);
    if (property_matches(node, name, p, n)) return;
    if (!bootpath_property && !bootargs_property && forthprop(node, name, p, n) && property_matches(node, name, p, n)) return;
  }
failed:
  print("Bootah OBP: handoff property "); print(name); print(" node="); hex(node); print("\r\n");
  fail("cannot publish OBP handoff context");
}
static void setprop(const char *name, const void *p, unsigned n) { nodeprop(chosen, name, p, n); }
static void claim(u64 address, u64 size) {
  u64 a[] = {(u64)"claim", 3, 1, address, size, 0, (u64)-1};
  if (cif(a) || a[6] != address) fail("cannot claim GRUB memory");
}
#include "network.h"
#include "handoff.h"
#include "native.h"
static void open_network(void) {
  long n = property(chosen, "bootpath", device, sizeof(device) - 1);
  if (n <= 0 || n >= (long)sizeof(device) - 1) fail("missing network bootpath");
  device[n] = 0;
  for (unsigned i = 0; device[i]; i++) if (device[i] == ':') { device[i] = 0; break; }
  u64 canonical[] = {(u64)"canon", 3, 1, (u64)device, (u64)bootpath, sizeof(bootpath) - 1, (u64)-1};
  if (cif(canonical) || (long)canonical[6] <= 0 || canonical[6] >= sizeof(device)) fail("cannot canonicalize boot device");
  bootpath[canonical[6]] = 0;
  unsigned size = length(bootpath);
  while (size && (bootpath[size-1] == ':' || bootpath[size-1] == ',')) bootpath[--size] = 0;
  memcpy(device, bootpath, size + 1);
  u64 node = find(device);
  char type[16] = {0};
  if (property(node, "device_type", type, sizeof(type)) < 0 || !equal(type, "network", 8))
    fail("bootpath is not a network device");
  char *p = bootpath; unsigned left = sizeof(bootpath);
  if (!path_text(&p, &left, device)) fail("network device path too long");
  char compatible[256] = {0};
  long count = property(node, "compatible", compatible, sizeof(compatible) - 1);
  if (count >= (long)sizeof(compatible)) fail("firmware compatible property too large");
  int vnet = 0;
  for (long i = 0; i < count;) {
    if (count - i >= 18 && equal(compatible + i, "SUNW,sun4v-network", 18)) vnet = 1;
    i += length(compatible + i) + 1;
  }
  if (!vnet && !path_text(&p, &left, ":1.1.1.1,dummy,1.1.1.1,1.1.1.1,5,5,255.255.255.0,512")) fail("network device path too long");
  *p = 0;
  u64 a[] = {(u64)"open", 1, 1, (u64)bootpath, 0};
  if (cif(a) || !a[4]) fail("cannot open firmware network interface");
  channel = a[4];
  if (property(node, "mac-address", mac, 6) != 6) {
    char policy[16] = {0};
    long policy_size = property(find("/options"), "local-mac-address?", policy, sizeof(policy));
    if (policy_size >= 5 && equal(policy, "false", 5)) {
      u64 m[] = {(u64)"interpret", 1, 3, (u64)"mac-address", (u64)-1, 0, 0};
      if (cif(m) || m[4] || m[5] != 6 || !m[6]) fail("system MAC address unavailable");
      memcpy(mac, (void *)m[6], 6);
    } else if (property(node, "local-mac-address", mac, 6) != 6) {
      fail("firmware does not report active MAC address");
    }
  }
  if ((mac[0] & 1) || equal(mac, "\0\0\0\0\0\0", 6)) fail("invalid firmware MAC address");
}
#ifdef OBP_HOST_TEST
extern void test_enter_stage(u64, u64) __attribute__((noreturn));
static void sync_code(u64 start, u64 size) { (void)start; (void)size; }
static void enter_stage(u64 target, u64 firmware) { test_enter_stage(target, firmware); }
#else
extern u8 stage_trampoline_start[], stage_trampoline_end[], _start[], image_end[];
static void sync_code(u64 start, u64 size) {
  for (u64 address = start & ~7UL; address < start + size; address += 8)
    __asm__ volatile("flush %0" :: "r"(address) : "memory");
  __asm__ volatile("membar #Sync" ::: "memory");
}
static void enter_stage(u64 target, u64 firmware) __attribute__((noreturn));
static void enter_stage(u64 target, u64 firmware) {
  u64 size = stage_trampoline_end - stage_trampoline_start;
  if (size > 8192) fail("handoff trampoline too large");
  u64 *args = (void *)0x5dc000;
  args[0] = target; args[1] = firmware; args[2] = original_stack;
  args[3] = (u64)(args + 8); args[4] = 2; args[5] = 0;
  args[6] = (u64)_start; args[7] = ((u64)(image_end - _start) + 8191) & ~8191UL;
  memcpy(args + 8, "release", 8);
  memcpy((void *)0x5de000, stage_trampoline_start, size);
  sync_code(0x5de000, size);
  ((void (*)(u64 *, u64))0x5de000)(args, original_stack);
  fail("next stage returned");
}
#endif
void loader_main(u64 entry, u64 stack) {
  cif = (void *)entry; original_stack = stack;
  chosen = find("/chosen"); output = handle("stdout");
  print("Bootah OBP/1 SPARC64\r\n");
  open_network();
  if (!dhcp_network()) fail("DHCP failed; no automatic firmware restart");
  if (!safe_bootfile(bootfile)) fail("unsafe or recursive next-stage filename");
  if (original_stack + 2047 >= 0x4000 && original_stack + 2047 < 0x4000 + GRUB_RESERVATION)
    fail("firmware stack overlaps GRUB runtime");
  claim(0x4000, GRUB_RESERVATION);
  /* Download away from the execution address, then validate before copying. */
  u8 *image = (u8 *)0x600000;
  u64 size = download(bootfile, image, GRUB_FILE_LIMIT);
  int native = size >= 12 && equal(image, "BOOTAH-OBP/1", 12);
  u64 target = GRUB_ADDRESS;
  if (native) {
    if (!native_plan(image, size, bootargs)) fail("invalid native boot plan");
    size = download(bootfile, image, GRUB_FILE_LIMIT);
    target = native_image(image, size);
    if (!target) fail("expected bounded SPARC64 ELF native loader");
    u64 table = be64(image + 32);
    for (unsigned i = 0; i < be16(image + 56); i++) {
      const u8 *p = image + table + i * 56;
      if (be32(p) != 1) continue;
      u64 address = be64(p + 16), file = be64(p + 32), memory = be64(p + 40);
      memcpy((void *)address, image + be64(p + 8), file);
      memset((void *)(address + file), 0, memory - file);
      sync_code(address, memory);
    }
  } else {
    if (!grub_image(image, size)) fail("expected SPARC64 GRUB a.out network image");
    memcpy((void *)GRUB_ADDRESS, image + 32, size - 32);
    sync_code(GRUB_ADDRESS, size - 32);
  }
  static u8 context[512] ALIGNED;
  unsigned context_size = handoff_packet(context);
  if (!handoff_path(bootpath, sizeof(bootpath), device, native)) fail("handoff path too long");
  if (!native) nodeprop(find(device), "mac-address", mac, 6);
  setprop("bootpath", bootpath, length(bootpath) + 1);
  setprop("bootargs", bootargs, length(bootargs) + 1);
  setprop("bootp-response", context, context_size);
  setprop("dhcp-response", context, context_size);
  u64 close_args[] = {(u64)"close", 1, 0, channel}; cif(close_args);
  print(native ? "Bootah OBP: entering native BSD loader\r\n" : "Bootah OBP: entering GRUB\r\n");
  enter_stage(target, entry);
}
