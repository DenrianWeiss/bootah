#include <assert.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
typedef uint64_t u64;
typedef uint32_t u32;
typedef uint16_t u16;
typedef uint8_t u8;
#define ALIGNED
static u8 rx[2048], tx[2048], mac[6] = {8, 0, 43, 229, 64, 0}, peer_mac[6];
static u32 ip, server, netmask, gateway, xid, lease_seconds;
static u16 source_port;
static u64 lease_started, ticks;
static unsigned scenario, discovers, boot_queries, resolved;
static u16 be16(const u8 *p) { return p[0] * 256U + p[1]; }
static u32 be32(const u8 *p) { return (u32)be16(p) * 65536 + be16(p + 2); }
static void put16(u8 *p, u16 v) {
  p[0] = v >> 8;
  p[1] = v;
}
static void put32(u8 *p, u32 v) {
  put16(p, v >> 16);
  put16(p + 2, v);
}
static int equal(const void *a, const void *b, size_t n) {
  return !memcmp(a, b, n);
}
static u64 now(void) { return ++ticks; }
static int expired(u64 start, unsigned seconds) {
  return now() - start >= seconds * 10U;
}
static void print(const char *s) { (void)s; }
static void hex(u64 v) { (void)v; }
static void fail(const char *s) {
  fprintf(stderr, "Unexpected loader failure: %s\n", s);
  abort();
}
static unsigned receive(void) { return 0; }
static void send_frame(unsigned n) { assert(n == 42); }
static void resolve(void) {
  assert(ip == 0x0a000278);
  resolved++;
  memset(peer_mac, 2, 6);
}
struct packet {
  u8 data[1024];
  unsigned n;
  u16 port;
  u32 sender;
};
static struct packet packets[32];
static unsigned head, tail;
static void opt(struct packet *p, u8 code, const void *bytes, unsigned n) {
  p->data[p->n++] = code;
  p->data[p->n++] = n;
  memcpy(p->data + p->n, bytes, n);
  p->n += n;
}
static struct packet *reply(u8 type, u32 sid, u32 address, u16 port, int pxe) {
  assert(tail < 32);
  struct packet *p = &packets[tail++];
  memset(p, 0, sizeof(*p));
  p->n = 240;
  p->port = port;
  p->sender = sid;
  p->data[0] = 2;
  p->data[1] = 1;
  p->data[2] = 6;
  put32(p->data + 4, xid);
  memcpy(p->data + 28, mac, 6);
  put32(p->data + 16, address);
  put32(p->data + 20, 0x0a000205);
  put32(p->data + 236, 0x63825363);
  opt(p, 53, &type, 1);
  u8 value[4];
  put32(value, sid);
  opt(p, 54, value, 4);
  if (address) {
    put32(value, 0xffffff00);
    opt(p, 1, value, 4);
    put32(value, 0x0a000201);
    opt(p, 3, value, 4);
    put32(value, 3600);
    opt(p, 51, value, 4);
  }
  if (pxe) {
    opt(p, 60, "PXEClient", 9);
    u8 vendor[] = {6, 1, 7, 8, 7, 0x80, 0, 1, 10, 0, 2, 4, 255};
    opt(p, 43, vendor, sizeof(vendor));
  }
  if (port == 4011 || scenario == 3)
    memcpy(p->data + 108, "bootah-srm.cfg", 15);
  return p;
}
static unsigned receive_udp(u16 port) {
  assert(port == 68);
  if (head == tail)
    return 0;
  struct packet *p = &packets[head++];
  memcpy(rx + 42, p->data, p->n);
  put16(rx + 34, p->port);
  put32(rx + 26, p->sender);
  return p->n;
}
static void udp(u32 dest, u16 port, const u8 *p, unsigned n, const u8 *target);
#include "dhcp.h"
static void udp(u32 dest, u16 port, const u8 *p, unsigned n, const u8 *target) {
  (void)target;
  u8 type[1], class[16], arch[2], sid[4];
  assert(dhcp_option(p, n, 53, type, 1) == 1);
  assert(dhcp_option(p, n, 77, class, sizeof(class)) == 11 && class[0] == 10 &&
         !memcmp(class + 1, "Bootah-SRM", 10));
  assert(dhcp_option(p, n, 93, arch, 2) == 2 && be16(arch) == 3);
  if (type[0] == 1) {
    assert(!ip && !be32(p + 12));
    discovers++;
    if (scenario == 4)
      return;
    if (scenario != 3 && scenario != 1)
      reply(2, 0x0a000202, 0, 67, 1);
    reply(2, 0x0a000203, 0x0a000278, 67, 0);
    if (scenario == 1)
      reply(2, 0x0a000202, 0, 67, 1);
  } else if (port == 67 && type[0] == 3) {
    assert(dhcp_option(p, n, 54, sid, 4) == 4 && be32(sid) == 0x0a000203);
    if (scenario == 2 && discovers == 1) {
      reply(6, 0x0a000203, 0, 67, 0);
      return;
    }
    reply(5, 0x0a000299, 0x0a000278, 67, 0);
    reply(5, 0x0a000203, 0x0a000279, 67, 0);
    reply(5, 0x0a000203, 0x0a000278, 67, 0);
  } else {
    assert(port == 4011 && type[0] == 3 && dest == 0x0a000204 &&
           be32(p + 12) == ip);
    assert(dhcp_option(p, n, 54, sid, 4) == 4 && be32(sid) == dest);
    boot_queries++;
    struct packet *bad = reply(5, dest, 0, 4011, 1);
    bad->data[4] ^= 1;
    reply(5, dest, 0, 4011, 1);
  }
}
int main(void) {
  for (scenario = 0; scenario < 5; scenario++) {
    ip = 0;
    head = tail = discovers = boot_queries = resolved = 0;
    ticks = 0;
    int ok = dhcp_network();
    if (scenario == 4) {
      assert(!ok && !ip);
      continue;
    }
    assert(ok && ip == 0x0a000278 && server == 0x0a000205 &&
           netmask == 0xffffff00 && gateway == 0x0a000201);
    assert(boot_queries == (scenario == 3 ? 0 : 1));
    assert(discovers == (scenario == 2 ? 2 : 1));
  }
  xid = 123;
  head = tail = 0;
  struct packet *p = reply(2, 0x0a000203, 0x0a000278, 67, 0);
  struct dhcp_reply r;
  assert(dhcp_parse(p->data, p->n, &r));
  p->data[28] ^= 1;
  assert(!dhcp_parse(p->data, p->n, &r));
  p->data[28] ^= 1;
  p->data[p->n++] = 52;
  p->data[p->n++] = 1;
  p->data[p->n++] = 1;
  memset(p->data + 108, 0, 128);
  p->data[108] = 67;
  p->data[109] = 3;
  memcpy(p->data + 110, "cfg", 3);
  p->data[113] = 255;
  assert(dhcp_parse(p->data, p->n, &r) && r.has_file);
  p->data[109] = 255;
  assert(!dhcp_parse(p->data, p->n, &r));
  puts("PASS: Proxy/DHCP ordering, NAK restart, selected ACK, 4011 identity, "
       "direct DHCP, missing offers, overload bounds, stage identity");
}
