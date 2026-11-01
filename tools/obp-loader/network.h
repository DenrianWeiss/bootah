/* Network transport adapted from Bootah SRM loader; firmware I/O is CIF. */
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
  long r = firmware_io("write", channel, tx, n);
  if (r != (long)n)
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
  long r = firmware_io("read", channel, rx, sizeof(rx));
  unsigned n = (unsigned)r;
  if (r < 0 || n > sizeof(rx) || n < 14)
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
  u64 count = 0, transfer_start = now();
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
      if (expired(transfer_start, 180)) fail("TFTP transfer deadline exceeded");
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

