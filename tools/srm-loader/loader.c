#include <stddef.h>
#include <stdint.h>

typedef uint64_t u64;
typedef uint32_t u32;
typedef uint16_t u16;
typedef uint8_t u8;
#define PAGE 8192UL
#define KSEG 0xfffffc0000000000UL
#define ALIGNED __attribute__((aligned(8)))
static volatile u64 *const hwrpb = (void *)0x10000000UL;
u64 dispatch_descriptor;
extern u64 srm_call(u64, u64, u64, u64, u64);
extern u64 osf_switch(u64, u64, u64, u64);
extern void kernel_enter(u64, u64) __attribute__((noreturn));
extern char _start[], _end[];
static u64 tty, channel, pcb[8] ALIGNED;
static u8 rx[2048] ALIGNED, tx[2048] ALIGNED, mac[6];
static u32 ip, server, netmask, gateway, xid;
static u16 source_port = 20000, peer_port;
static u8 peer_mac[6];
static char device[256] ALIGNED, flags[256] ALIGNED;
static u64 ticks_per_second, last_cycle, clock_ticks, lease_started;
static u32 lease_seconds;
static u64 reserved_lo[512], reserved_hi[512];
static unsigned reservations;
struct memory_region {
  u64 pfn, pages, tested, bitmap, usage;
};
static struct memory_region memory[256];
static unsigned regions;
typedef u64 alias_u64 __attribute__((may_alias));
struct __attribute__((packed, may_alias)) unaligned_u64 {
  u64 value;
};

void *memcpy(void *d, const void *s, size_t n) {
  u8 *a = d;
  const u8 *b = s;
  while (n && ((u64)a & 7)) {
    *a++ = *b++;
    n--;
  }
  while (n >= 8) {
    *(alias_u64 *)a = ((const struct unaligned_u64 *)b)->value;
    a += 8;
    b += 8;
    n -= 8;
  }
  while (n--)
    *a++ = *b++;
  return d;
}
void *memset(void *d, int c, size_t n) {
  u8 *a = d;
  while (n && ((u64)a & 7)) {
    *a++ = (u8)c;
    n--;
  }
  u64 fill = (u8)c * 0x0101010101010101UL;
  while (n >= 8) {
    *(alias_u64 *)a = fill;
    a += 8;
    n -= 8;
  }
  while (n--)
    *a++ = (u8)c;
  return d;
}
static size_t length(const char *s) {
  size_t n = 0;
  while (s[n])
    n++;
  return n;
}
static int equal(const void *a, const void *b, size_t n) {
  const u8 *x = a, *y = b;
  while (n--)
    if (*x++ != *y++)
      return 0;
  return 1;
}
static u16 be16(const u8 *p) { return (u16)p[0] * 256 + p[1]; }
static u32 be32(const u8 *p) { return (u32)be16(p) * 65536 + be16(p + 2); }
static void put16(u8 *p, u16 v) {
  p[0] = v >> 8;
  p[1] = v;
}
static void put32(u8 *p, u32 v) {
  put16(p, v >> 16);
  put16(p + 2, v);
}
static u64 number(const char *s) {
  u64 n = 0;
  while (*s >= '0' && *s <= '9')
    n = n * 10 + *s++ - '0';
  return n;
}
static void print(const char *s) {
  static char out[256] ALIGNED;
  while (*s) {
    unsigned n = 0;
    while (*s && n < 250) {
      if (*s == '\n')
        out[n++] = '\r';
      out[n++] = *s++;
    }
    unsigned done = 0;
    for (unsigned retry = 0; done < n && retry < 10000; retry++) {
      u64 r = srm_call(2, tty, (u64)(out + done), n - done, 0);
      unsigned count = (u32)r;
      if (count > n - done)
        break;
      done += count;
    }
  }
}
static void hex(u64 n) {
  char b[19];
  b[0] = '0';
  b[1] = 'x';
  for (int i = 0; i < 16; i++)
    b[2 + i] = "0123456789abcdef"[(n >> (60 - i * 4)) & 15];
  b[18] = 0;
  print(b);
}
static void fail(const char *s) __attribute__((noreturn));
static void fail(const char *s) {
  print("Bootah SRM: ");
  print(s);
  print("\n");
  for (;;)
    __asm__ volatile("call_pal 0");
}
static void env(u64 id, char *buffer, unsigned capacity) {
  u64 r = srm_call(0x22, id, (u64)buffer, capacity - 1, 0);
  unsigned n = (u32)r;
  if ((r >> 63) || n >= capacity)
    n = 0;
  buffer[n] = 0;
}
static u64 now(void) {
  u64 c;
  __asm__ volatile("rpcc %0" : "=r"(c));
  c = (u32)c;
  clock_ticks += (u32)(c - last_cycle);
  last_cycle = c;
  return clock_ticks;
}
static int expired(u64 since, unsigned seconds) {
  return now() - since >= ticks_per_second * seconds;
}
static u64 physical(u64 va) {
  if (va >= KSEG)
    return va - KSEG;
  volatile u64 *vpt = (void *)hwrpb[15];
  u64 pte = vpt[va >> 13];
  if (!(pte & 1))
    fail("unmapped bootstrap address");
  return ((pte >> 32) << 13) | (va & (PAGE - 1));
}
static void reserve(u64 lo, u64 size) {
  if (!size || lo + size < lo)
    fail("invalid reserved memory range");
  u64 hi = lo + size;
  for (unsigned i = 0; i < reservations;) {
    if (lo <= reserved_hi[i] && reserved_lo[i] <= hi) {
      if (reserved_lo[i] < lo)
        lo = reserved_lo[i];
      if (reserved_hi[i] > hi)
        hi = reserved_hi[i];
      reservations--;
      reserved_lo[i] = reserved_lo[reservations];
      reserved_hi[i] = reserved_hi[reservations];
      i = 0;
    } else
      i++;
  }
  if (reservations == 512)
    fail("too many bootstrap memory ranges");
  reserved_lo[reservations] = lo;
  reserved_hi[reservations++] = hi;
}
static void reserve_virtual(u64 va, u64 size) {
  u64 end = (va + size + PAGE - 1) & -PAGE;
  for (va &= -PAGE; va < end; va += PAGE)
    reserve(physical(va), PAGE);
}
static int free_range(u64 lo, u64 size) {
  for (unsigned i = 0; i < reservations; i++)
    if (lo < reserved_hi[i] && reserved_lo[i] < lo + size)
      return 0;
  return 1;
}
static void add_memory(u64 pfn, u64 pages, u64 tested, u64 bitmap, u64 usage) {
  if (regions == 256 || tested > pages || pfn > (~0UL / PAGE) - pages)
    fail("invalid memory descriptor");
  memory[regions++] = (struct memory_region){pfn, pages, tested, bitmap, usage};
  if (bitmap)
    reserve(bitmap, (tested + 7) / 8);
}
static void memory_descriptors(void) {
  u64 *m = (void *)((u64)hwrpb + hwrpb[25]);
  if (m[2] > 256)
    fail("unsupported memory descriptor count");
  if (m[2] == 1 && m[3] == ~0UL) {
    if (hwrpb[2] < 12 || !hwrpb[27])
      fail("invalid distributed memory descriptors");
    u64 fru = hwrpb[0] + hwrpb[27];
    for (u64 offset = m[7]; offset != ~0UL;) {
      u64 pa = fru + offset;
      if (pa & 7 || pa >= -KSEG)
        fail("invalid distributed memory descriptor offset");
      u64 *d = (void *)(KSEG + pa);
      reserve(pa, 80);
      u32 *fields = (void *)(d + 2);
      add_memory(fields[0], fields[1], fields[2], d[4], fields[3]);
      offset = d[1];
    }
    return;
  }
  for (unsigned i = 0; i < m[2]; i++) {
    u64 *c = m + 3 + i * 7;
    add_memory(c[0], c[1], c[2], c[4], c[6]);
  }
}
static int tested_range(const struct memory_region *c, u64 lo, u64 size) {
  u64 base = c->pfn * PAGE, top = base + c->tested * PAGE;
  if ((c->usage & 3) || lo < base || lo + size < lo || lo + size > top)
    return 0;
  if (c->bitmap) {
    const u8 *bitmap = (void *)(KSEG + c->bitmap);
    for (u64 j = (lo - base) / PAGE; j < (lo + size - base + PAGE - 1) / PAGE;
         j++)
      if (!(bitmap[j / 8] & (1 << (j % 8))))
        return 0;
  }
  return 1;
}
static int usable_range(u64 lo, u64 size) {
  for (unsigned i = 0; i < regions; i++)
    if (tested_range(memory + i, lo, size))
      return 1;
  return 0;
}
static u64 allocate(u64 size) {
  size = (size + PAGE - 1) & -PAGE;
  for (unsigned i = 0; i < regions; i++) {
    const struct memory_region *c = memory + i;
    if (c->usage & 3)
      continue;
    u64 lo = c->pfn * PAGE;
    u64 hi = lo + c->tested * PAGE;
    if (hi < lo || hi < size)
      continue;
    for (u64 p = (hi - size) & -PAGE; p >= lo && p >= PAGE; p -= PAGE) {
      if (!free_range(p, size))
        continue;
      if (tested_range(c, p, size)) {
        reserve(p, size);
        return KSEG + p;
      }
    }
  }
  fail("insufficient tested contiguous memory");
}
static void setup_memory(void) {
  if (hwrpb[5] != PAGE || hwrpb[15] != 0x200000000UL)
    fail("unsupported SRM page geometry");
  u64 sp;
  __asm__ volatile("mov $30,%0" : "=r"(sp));
  reserve_virtual((u64)_start, (u64)(_end - _start));
  reserve_virtual(sp & -PAGE, PAGE);
  reserve_virtual((u64)hwrpb, hwrpb[3]);
  volatile u64 *l1 = (void *)0x200802000UL;
  reserve((l1[1] >> 32) * PAGE, PAGE);
  /* The architected initial L1 has two valid entries, including its self map.
   */
  volatile u64 *l2 = (void *)(KSEG + (l1[0] >> 32) * PAGE);
  reserve((l1[0] >> 32) * PAGE, PAGE);
  for (unsigned i = 0; i < 1024; i++)
    if (l2[i] & 1) {
      u64 pa = (l2[i] >> 32) * PAGE;
      reserve(pa, PAGE);
      if (pa == (l1[0] >> 32) * PAGE)
        continue;
      volatile u64 *l3 = (void *)(KSEG + pa);
      for (unsigned j = 0; j < 1024; j++)
        if (l3[j] & 1)
          reserve((l3[j] >> 32) * PAGE, PAGE);
    }
}
static u16 checksum(const u8 *p, unsigned n) {
  u32 sum = 0;
  while (n > 1) {
    sum += be16(p);
    p += 2;
    n -= 2;
  }
  if (n)
    sum += (u16)*p << 8;
  while (sum >> 16)
    sum = (sum & 65535) + (sum >> 16);
  return (u16)~sum;
}
static void send_frame(unsigned n) {
  if (n < 60) {
    memset(tx + n, 0, 60 - n);
    n = 60;
  }
  u64 r = srm_call(0x14, channel, n, (u64)tx, 0);
  if (r >> 63)
    fail("firmware network WRITE failed");
}
static void udp(u32 destination, u16 port, const u8 *data, unsigned size,
                const u8 *target) {
  if (size > 1472)
    fail("oversized UDP packet");
  memset(tx, 0, 42);
  memcpy(tx, target, 6);
  memcpy(tx + 6, mac, 6);
  put16(tx + 12, 0x800);
  tx[14] = 0x45;
  put16(tx + 16, 28 + size);
  tx[22] = 64;
  tx[23] = 17;
  put32(tx + 26, ip);
  put32(tx + 30, destination);
  put16(tx + 24, checksum(tx + 14, 20));
  put16(tx + 34, source_port);
  put16(tx + 36, port);
  put16(tx + 38, 8 + size);
  memcpy(tx + 42, data, size);
  send_frame(42 + size);
}
static unsigned receive(void) {
  if (lease_seconds && expired(lease_started, lease_seconds))
    fail("DHCP lease expired before kernel handoff");
  u64 r = srm_call(0x13, channel, sizeof(rx), (u64)rx, 0);
  unsigned n = (u32)r;
  if ((r >> 63) || n > sizeof(rx) || n < 14)
    return 0;
  if (be16(rx + 12) == 0x806 && n >= 42 && be16(rx + 14) == 1 &&
      be16(rx + 16) == 0x800 && rx[18] == 6 && rx[19] == 4) {
    if (be16(rx + 20) == 1 && ip && be32(rx + 38) == ip) {
      memcpy(tx, rx + 6, 6);
      memcpy(tx + 6, mac, 6);
      put16(tx + 12, 0x806);
      memcpy(tx + 14, rx + 14, 28);
      put16(tx + 20, 2);
      memcpy(tx + 22, mac, 6);
      put32(tx + 28, ip);
      memcpy(tx + 32, rx + 22, 6);
      memcpy(tx + 38, rx + 28, 4);
      send_frame(42);
    }
  }
  return n;
}
static unsigned receive_udp(u16 port) {
  unsigned n = receive();
  if (n < 42 || be16(rx + 12) != 0x800 || rx[14] != 0x45 || rx[23] != 17 ||
      checksum(rx + 14, 20) || (be16(rx + 20) & 0x3fff))
    return 0;
  unsigned total = be16(rx + 16), len = be16(rx + 38);
  if (total < 28 || total + 14 > n || len < 8 || len + 20 > total ||
      be16(rx + 36) != port)
    return 0;
  if (ip && be32(rx + 30) != ip && be32(rx + 30) != 0xffffffff)
    return 0;
  if (be16(rx + 40)) {
    u32 sum = 0;
    for (unsigned i = 26; i < 34; i += 2)
      sum += be16(rx + i);
    sum += 17 + len;
    for (unsigned i = 34; i < 34 + len; i += 2)
      sum += i + 1 < 34 + len ? be16(rx + i) : (u16)rx[i] << 8;
    while (sum >> 16)
      sum = (sum & 65535) + (sum >> 16);
    if (sum != 65535)
      return 0;
  }
  return len - 8;
}
static void resolve(void) {
  u32 next = (ip & netmask) == (server & netmask) ? server : gateway;
  if (!next)
    fail("missing gateway for boot server");
  for (unsigned retry = 0; retry < 5; retry++) {
    memset(tx, 0, 60);
    memset(tx, 255, 6);
    memcpy(tx + 6, mac, 6);
    put16(tx + 12, 0x806);
    put16(tx + 14, 1);
    put16(tx + 16, 0x800);
    tx[18] = 6;
    tx[19] = 4;
    put16(tx + 20, 1);
    memcpy(tx + 22, mac, 6);
    put32(tx + 28, ip);
    put32(tx + 38, next);
    send_frame(42);
    u64 start = now();
    while (!expired(start, 2))
      if (receive() >= 42 && be16(rx + 12) == 0x806 && be16(rx + 20) == 2 &&
          be32(rx + 28) == next && be32(rx + 38) == ip) {
        memcpy(peer_mac, rx + 22, 6);
        return;
      }
  }
  fail("boot server ARP timed out");
}
#include "dhcp.h"
static void bootp_network(void) {
  static u8 request[300] ALIGNED;
  static const u8 broadcast[6] = {255, 255, 255, 255, 255, 255};
  memset(request, 0, sizeof(request));
  request[0] = 1;
  request[1] = 1;
  request[2] = 6;
  xid = (u32)now() ^ 0x42414853;
  put32(request + 4, xid);
  memcpy(request + 28, mac, 6);
  put32(request + 236, 0x63825363);
  request[240] = 255;
  source_port = 68;
  for (unsigned retry = 0; retry < 6; retry++) {
    put16(request + 8, retry * 3);
    udp(0xffffffff, 67, request, sizeof(request), broadcast);
    u64 start = now();
    while (!expired(start, 3)) {
      unsigned n = receive_udp(68);
      const u8 *p = rx + 42;
      if (n < 240 || p[0] != 2 || p[1] != 1 || p[2] != 6 ||
          be32(p + 4) != xid || !equal(p + 28, mac, 6))
        continue;
      ip = be32(p + 16);
      server = be32(p + 20);
      netmask = 0;
      gateway = be32(p + 24);
      for (unsigned pos = 240; pos < n;) {
        unsigned tag = p[pos++];
        if (tag == 255)
          break;
        if (!tag)
          continue;
        if (pos >= n)
          break;
        unsigned len = p[pos++];
        if (len > n - pos)
          break;
        if (tag == 1 && len == 4)
          netmask = be32(p + pos);
        if (tag == 3 && len >= 4)
          gateway = be32(p + pos);
        pos += len;
      }
      if (!ip || !server)
        fail("BOOTP reply lacks client or boot server address");
      if (!netmask)
        fail("BOOTP reply lacks subnet mask");
      print("BOOTP client=");
      hex(ip);
      print(" server=");
      hex(server);
      print("\n");
      source_port = 20000;
      resolve();
      return;
    }
  }
  fail("BOOTP timed out; external DHCP must answer BOOTP or use a reservation");
}
static void network(void) {
  if (!dhcp_network()) {
    print("DHCP unavailable; trying legacy BOOTP\n");
    bootp_network();
  }
}
static u64 download(const char *name, u8 *destination, u64 capacity) {
  static u8 request[512] ALIGNED;
  unsigned path = length(name);
  if (!path || path > 240)
    fail("invalid TFTP filename");
  print("TFTP ");
  print(name);
  print("\n");
  request[0] = 0;
  request[1] = 1;
  memcpy(request + 2, name, path + 1);
  static const char options[] = "octet\0blksize\0"
                                "1400\0windowsize\0"
                                "4";
  memcpy(request + 3 + path, options, sizeof(options));
  unsigned request_size = 3 + path + sizeof(options);
  source_port++;
  peer_port = 0;
  u16 block = 0;
  unsigned blocksize = 512;
  unsigned windowsize = 1;
  u64 count = 0;
  u8 ack[4] = {0, 4, 0, 0};
  for (unsigned retries = 0; retries < 6;) {
    if (!peer_port)
      udp(server, 69, request, request_size, peer_mac);
    else {
      put16(ack + 2, block);
      udp(server, peer_port, ack, 4, peer_mac);
    }
    u64 start = now();
    int received = 0;
    unsigned window_blocks = 0;
    while (!expired(start, 3)) {
      unsigned n = receive_udp(source_port);
      const u8 *p = rx + 42;
      if (n < 2 || be32(rx + 26) != server ||
          (peer_port && be16(rx + 34) != peer_port))
        continue;
      u16 opcode = be16(p);
      if (opcode == 5)
        fail("TFTP server rejected file");
      if (opcode == 6 && !peer_port && !count) {
        if (n < 4 || p[n - 1] != 0)
          fail("invalid TFTP option response");
        unsigned seen = 0;
        for (unsigned at = 2; at < n;) {
          const char *key = (const char *)p + at;
          unsigned k = 0;
          while (at + k < n && p[at + k])
            k++;
          if (at + k + 1 >= n)
            fail("truncated TFTP option");
          at += k + 1;
          u64 value = 0;
          unsigned digits = 0;
          while (at < n && p[at]) {
            if (p[at] < '0' || p[at] > '9' || digits++ > 5)
              fail("invalid TFTP option number");
            value = value * 10 + p[at++] - '0';
          }
          if (at == n || !digits)
            fail("invalid TFTP option number");
          at++;
          if (k == 7 && equal(key, "blksize", 7) && !(seen & 1)) {
            if (value < 8 || value > 1400)
              fail("unsupported TFTP block size");
            blocksize = value;
            seen |= 1;
          } else if (k == 10 && equal(key, "windowsize", 10) && !(seen & 2)) {
            if (value < 1 || value > 4)
              fail("unsupported TFTP window size");
            windowsize = value;
            seen |= 2;
          } else
            fail("unrequested or duplicate TFTP option");
        }
        peer_port = be16(rx + 34);
        received = 1;
        break;
      }
      if (opcode != 3 || n < 4 || n > blocksize + 4)
        continue;
      u16 next = be16(p + 2);
      if (next == block && peer_port) {
        received = 1;
        break;
      }
      if (next != (u16)(block + 1)) {
        received = 1;
        break;
      }
      if (!peer_port)
        peer_port = be16(rx + 34);
      if (n - 4 > capacity - count)
        fail("TFTP file exceeds declared size");
      memcpy(destination + count, p + 4, n - 4);
      if ((count >> 20) != ((count + n - 4) >> 20)) {
        print("TFTP progress=");
        hex(count + n - 4);
        print("\n");
      }
      count += n - 4;
      block = next;
      if (n - 4 < blocksize) {
        put16(ack + 2, block);
        udp(server, peer_port, ack, 4, peer_mac);
        print("TFTP bytes=");
        hex(count);
        print("\n");
        return count;
      }
      received = 1;
      if (++window_blocks >= windowsize)
        break;
    }
    if (received)
      retries = 0;
    else
      retries++;
  }
  fail("TFTP timed out");
}

struct elf_header {
  u8 id[16];
  u16 type, machine;
  u32 version;
  u64 entry, phoff, shoff;
  u32 flags;
  u16 ehsize, phsize, phnum, shsize, shnum, shstr;
};
struct elf_segment {
  u32 type, flags;
  u64 offset, va, pa, filesz, memsz, align;
};
static char config[2048] ALIGNED;
static char *setting(const char *key) {
  size_t k = length(key);
  for (char *p = config; *p; p += length(p) + 1)
    if (equal(p, key, k) && p[k] == '=')
      return p + k + 1;
  fail("missing boot configuration field");
}
static void boot_linux(void) {
  u64 size = download("bootah-srm.cfg", (u8 *)config, sizeof(config) - 2);
  if (size < 14 || !equal(config, "BOOTAH-SRM 1\n", 13))
    fail("unsupported boot configuration");
  config[size] = 0;
  config[size + 1] = 0;
  for (u64 i = 0; i < size; i++)
    if (config[i] == '\n')
      config[i] = 0;
  u64 kernel_size = number(setting("kernel_size")),
      initrd_size = number(setting("initrd_size"));
  if (kernel_size < 64 || kernel_size > 512UL * 1024 * 1024 ||
      initrd_size > 1024UL * 1024 * 1024)
    fail("invalid payload sizes");
  u64 image = allocate(kernel_size);
  if (download(setting("kernel"), (u8 *)image, kernel_size) != kernel_size)
    fail("incomplete kernel");
  struct elf_header *e = (void *)image;
  static const u8 ident[] = {127, 'E', 'L', 'F', 2, 1, 1};
  if (!equal(e->id, ident, 7) || e->machine != 0x9026 || e->type != 2 ||
      e->phsize != sizeof(struct elf_segment) || !e->phnum || e->phnum > 32 ||
      e->phoff > kernel_size || e->phnum * e->phsize > kernel_size - e->phoff)
    fail("not a supported Alpha ELF64 kernel");
  if (e->entry < KSEG + 0x10000 || e->entry >= KSEG + 0x100000000UL)
    fail("invalid Linux entry address");
  u64 entry = e->entry, zero = entry - 0x6000, stack = entry - 0xe000;
  struct elf_segment *segments = (void *)(image + e->phoff);
  unsigned executable_entry = 0;
  for (unsigned i = 0; i < e->phnum; i++) {
    struct elf_segment *s = segments + i;
    if (s->type != 1)
      continue;
    if (s->va < KSEG || s->memsz < s->filesz || s->offset > kernel_size ||
        s->filesz > kernel_size - s->offset || s->va + s->memsz < s->va ||
        s->va + s->memsz > KSEG + 0x100000000UL)
      fail("invalid kernel segment");
    if (!s->memsz)
      continue;
    if (!usable_range(s->va - KSEG, s->memsz))
      fail("kernel segment is outside tested usable RAM");
    if (!free_range(s->va - KSEG, s->memsz))
      fail("kernel overlaps active firmware or loader memory");
    if ((s->flags & 1) && entry >= s->va && entry - s->va < s->memsz)
      executable_entry = 1;
    reserve(s->va - KSEG, s->memsz);
  }
  if (!executable_entry)
    fail("kernel entry is outside executable segments");
  /* Linux's boot parameter page and initial thread stack are relative to entry.
   */
  u64 initrd = initrd_size ? allocate(initrd_size) : 0;
  if (initrd_size &&
      download(setting("initrd"), (u8 *)initrd, initrd_size) != initrd_size)
    fail("incomplete initrd");
  char args[256];
  char *configured = setting("args");
  unsigned base_length = length(configured), flag_length = length(flags);
  unsigned separator = base_length && flag_length;
  if (base_length + separator + flag_length > 255)
    fail("Linux command line exceeds 255 bytes");
  memcpy(args, configured, base_length);
  if (separator)
    args[base_length] = ' ';
  memcpy(args + base_length + separator, flags, flag_length + 1);
  print("Linux entry=");
  hex(entry);
  print(" initrd=");
  hex(initrd);
  print("\n");
  srm_call(0x11, channel, 0, 0, 0);
  for (unsigned i = 0; i < e->phnum; i++) {
    struct elf_segment *s = segments + i;
    if (s->type != 1)
      continue;
    memcpy((void *)s->va, (void *)(image + s->offset), s->filesz);
    memset((void *)(s->va + s->filesz), 0, s->memsz - s->filesz);
  }
  memset((void *)zero, 0, PAGE);
  memcpy((void *)zero, args, length(args) + 1);
  ((u64 *)zero)[32] = initrd;
  ((u64 *)zero)[33] = initrd_size;
  print("Kernel loaded; entering Linux\n");
  kernel_enter(entry, stack);
}
void loader_main(void) {
  if (hwrpb[1] != 0x0000004250525748UL)
    for (;;)
      __asm__ volatile("call_pal 0");
  u64 *crb = (void *)((u64)hwrpb + hwrpb[24]);
  dispatch_descriptor = crb[0];
  static char terminal[16] ALIGNED;
  env(15, terminal, sizeof(terminal));
  tty = number(terminal);
  print("Bootah SRM loader 0.1\nSystem type=");
  hex(hwrpb[10]);
  print(" page size=");
  hex(hwrpb[5]);
  print("\n");
  ticks_per_second = hwrpb[14];
  if (!ticks_per_second)
    fail("missing CPU cycle frequency");
  now();
  env(4, device, sizeof(device));
  env(8, flags, sizeof(flags));
  print("Boot device: ");
  print(device);
  print("\n");
  char *address = device;
  for (unsigned i = 0; i < 8; i++) {
    while (*address && *address != ' ')
      address++;
    if (!*address)
      fail("boot device lacks MAC address");
    address++;
  }
  for (unsigned i = 0; i < 6; i++) {
    unsigned n = 0;
    for (unsigned j = 0; j < 2; j++) {
      char c = *address++;
      unsigned v = c >= '0' && c <= '9'   ? c - '0'
                   : c >= 'A' && c <= 'F' ? c - 'A' + 10
                   : c >= 'a' && c <= 'f' ? c - 'a' + 10
                                          : 255;
      if (v > 15)
        fail("invalid boot device MAC address");
      n = n * 16 + v;
    }
    mac[i] = n;
    if (i < 5) {
      if (*address != '-' && *address != ':')
        fail("invalid MAC separator");
      address++;
    }
  }
  if (hwrpb[5] != PAGE || hwrpb[15] != 0x200000000UL)
    fail("unsupported page geometry");
  pcb[5] = 1;
  u64 result = osf_switch(physical((u64)pcb), (*(u64 *)0x200802008UL) >> 32,
                          hwrpb[15], (u64)pcb);
  if (result)
    fail("OSF PALcode switch failed");
  u64 *cpu = (void *)((u64)hwrpb + hwrpb[20] + hwrpb[4] * hwrpb[19]);
  cpu[21] = cpu[58 + 2];
  setup_memory();
  memory_descriptors();
  u64 opened = srm_call(0x10, (u64)device, length(device), 0, 0);
  if (opened >> 62)
    fail("cannot open SRM boot network device");
  channel = (u32)opened;
  network();
  boot_linux();
}
