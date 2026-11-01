/* DHCP/PXE state machine adapted from Bootah SRM loader. */
struct dhcp_reply {
  u32 address, identifier, boot, mask, router, lease, discovery;
  u16 boot_type, boot_item;
  u8 message, pxe, control, has_file, has_item;
  char filename[241];
};
static int ipv4_usable(u32 a) {
  return a && (a >> 24) && (a >> 24) < 224 && a != 0xffffffff;
}
static u32 ipv4_literal(const u8 *text, unsigned n) {
  u32 result = 0;
  unsigned at = 0;
  for (unsigned part = 0; part < 4; part++) {
    unsigned value = 0, digits = 0;
    while (at < n && text[at] >= '0' && text[at] <= '9') {
      value = value * 10 + text[at++] - '0';
      if (++digits > 3 || value > 255)
        return 0;
    }
    if (!digits)
      return 0;
    result = (result << 8) | value;
    if (part < 3 && (at == n || text[at++] != '.'))
      return 0;
  }
  return at == n || (at + 1 == n && !text[at]) ? result : 0;
}
static int option_part(const u8 *p, unsigned n, unsigned tag, u8 *out,
                       unsigned capacity, unsigned *used) {
  for (unsigned i = 0; i < n;) {
    unsigned code = p[i++];
    if (code == 255)
      return 1;
    if (!code)
      continue;
    if (i == n)
      return 0;
    unsigned size = p[i++];
    if (size > n - i)
      return 0;
    if (code == tag) {
      if (size > capacity - *used)
        return 0;
      memcpy(out + *used, p + i, size);
      *used += size;
    }
    i += size;
  }
  return 1;
}
static int dhcp_option(const u8 *p, unsigned n, unsigned tag, u8 *out,
                       unsigned capacity) {
  unsigned used = 0, overloaded = 0;
  u8 overload[1];
  if (n < 240 || be32(p + 236) != 0x63825363 ||
      !option_part(p + 240, n - 240, 52, overload, 1, &overloaded) ||
      (overloaded && (overload[0] < 1 || overload[0] > 3)))
    return -1;
  if (!option_part(p + 240, n - 240, tag, out, capacity, &used) ||
      (overloaded && (overload[0] & 1) &&
       !option_part(p + 108, 128, tag, out, capacity, &used)) ||
      (overloaded && (overload[0] & 2) &&
       !option_part(p + 44, 64, tag, out, capacity, &used)))
    return -1;
  return (int)used;
}
static int dhcp_parse(const u8 *p, unsigned n, struct dhcp_reply *r) {
  u8 value[512];
  if (n < 240 || p[0] != 2 || p[1] != 1 || p[2] != 6 || be32(p + 4) != xid ||
      !equal(p + 28, mac, 6))
    return 0;
  memset(r, 0, sizeof(*r));
  r->address = be32(p + 16);
  r->boot = be32(p + 20);
  int count = dhcp_option(p, n, 53, value, 1);
  if (count != 1)
    return 0;
  r->message = value[0];
  count = dhcp_option(p, n, 54, value, 4);
  if (count != 4 || !ipv4_usable(r->identifier = be32(value)))
    return 0;
  unsigned tags[] = {1, 3, 51};
  u32 *fields[] = {&r->mask, &r->router, &r->lease};
  for (unsigned i = 0; i < 3; i++) {
    count = dhcp_option(p, n, tags[i], value, sizeof(value));
    if (count < 0 ||
        (count && (count < 4 || count % 4 || (tags[i] != 3 && count != 4))))
      return 0;
    if (count)
      *fields[i] = be32(value);
  }
  count = dhcp_option(p, n, 60, value, sizeof(value));
  if (count < 0)
    return 0;
  r->pxe = count >= 9 && equal(value, "PXEClient", 9);
  count = dhcp_option(p, n, 66, value, sizeof(value));
  if (count < 0)
    return 0;
  if (!r->boot && count)
    r->boot = ipv4_literal(value, count);
  count = dhcp_option(p, n, 67, value, sizeof(value));
  if (count < 0)
    return 0;
  r->has_file = count > 0 && value[0];
  if (r->has_file) {
    if (count > 240) return 0;
    memcpy(r->filename, value, count);
    r->filename[count] = 0;
  }
  count = dhcp_option(p, n, 52, value, 1);
  if (count < 0)
    return 0;
  if (!r->has_file && !(count && (value[0] & 1))) {
    unsigned len = 0;
    while (len < 128 && p[108 + len]) len++;
    if (len == 128) return 0;
    memcpy(r->filename, p + 108, len);
    r->has_file = len != 0;
  }
  count = dhcp_option(p, n, 43, value, sizeof(value));
  if (count < 0)
    return 0;
  if (!r->pxe)
    return 1;
  for (unsigned i = 0; i < (unsigned)count;) {
    unsigned tag = value[i++];
    if (tag == 255)
      break;
    if (!tag)
      continue;
    if (i == (unsigned)count)
      return 0;
    unsigned size = value[i++];
    if (size > (unsigned)count - i)
      return 0;
    if (tag == 6) {
      if (size != 1)
        return 0;
      r->control = value[i];
    } else if (tag == 8) {
      for (unsigned at = i; at < i + size;) {
        if (i + size - at < 3)
          return 0;
        unsigned addresses = value[at + 2];
        if (!addresses || addresses > (i + size - at - 3) / 4)
          return 0;
        if (!r->discovery) {
          r->boot_type = be16(value + at);
          r->discovery = be32(value + at + 3);
          if (!ipv4_usable(r->discovery))
            return 0;
        }
        at += 3 + addresses * 4;
      }
    } else if (tag == 71) {
      if (size != 4 || be16(value + i + 2))
        return 0;
      r->boot_item = be16(value + i);
      r->has_item = 1;
    }
    i += size;
  }
  return 1;
}
static unsigned dhcp_request(u8 *p, u8 message, u32 selected, u32 address,
                             u16 boot_type, int boot_service) {
  memset(p, 0, 512);
  p[0] = 1;
  p[1] = 1;
  p[2] = 6;
  put32(p + 4, xid);
  if (!boot_service && (message == 1 || message == 3)) p[10] = 0x80;
  if (boot_service)
    put32(p + 12, ip);
  memcpy(p + 28, mac, 6);
  put32(p + 236, 0x63825363);
  unsigned n = 240;
  p[n++] = 53;
  p[n++] = 1;
  p[n++] = message;
  p[n++] = 61;
  p[n++] = 7;
  p[n++] = 1;
  memcpy(p + n, mac, 6);
  n += 6;
  static const char stage2[] = "Bootah-OBP/1";
  p[n++] = 77;
  p[n++] = sizeof(stage2);
  p[n++] = sizeof(stage2) - 1;
  memcpy(p + n, stage2, sizeof(stage2) - 1);
  n += sizeof(stage2) - 1;
  static const char vendor[] = "PXEClient";
  p[n++] = 60;
  p[n++] = sizeof(vendor) - 1;
  memcpy(p + n, vendor, sizeof(vendor) - 1);
  n += sizeof(vendor) - 1;
  p[n++] = 57;
  p[n++] = 2;
  put16(p + n, 1472);
  n += 2;
  static const u8 requested[] = {1, 3, 43, 51, 54, 60, 66, 67};
  p[n++] = 55;
  p[n++] = sizeof(requested);
  memcpy(p + n, requested, sizeof(requested));
  n += sizeof(requested);
  if (selected) {
    p[n++] = 54;
    p[n++] = 4;
    put32(p + n, selected);
    n += 4;
  }
  if (address) {
    p[n++] = 50;
    p[n++] = 4;
    put32(p + n, address);
    n += 4;
  }
  if (boot_service) {
    p[n++] = 43;
    p[n++] = 7;
    p[n++] = 71;
    p[n++] = 4;
    put16(p + n, boot_type);
    n += 2;
    put16(p + n, 0);
    n += 2;
    p[n++] = 255;
  }
  p[n++] = 255;
  return n;
}
static int dhcp_receive(u16 port, struct dhcp_reply *r) {
  unsigned n = receive_udp(68);
  return n && be16(rx + 34) == port && dhcp_parse(rx + 42, n, r);
}
static void dhcp_claim(u8 *request, u32 identifier) {
  static const u8 broadcast[6] = {255, 255, 255, 255, 255, 255};
  u32 candidate = ip;
  ip = 0;
  for (unsigned probe = 0; probe < 2; probe++) {
    memset(tx, 0, 60);
    memcpy(tx, broadcast, 6);
    memcpy(tx + 6, mac, 6);
    put16(tx + 12, 0x806);
    put16(tx + 14, 1);
    put16(tx + 16, 0x800);
    tx[18] = 6;
    tx[19] = 4;
    put16(tx + 20, 1);
    memcpy(tx + 22, mac, 6);
    put32(tx + 38, candidate);
    send_frame(42);
    u64 start = now();
    while (!expired(start, 1)) {
      if (receive() >= 42 && be16(rx + 12) == 0x806 && be16(rx + 14) == 1 &&
          be16(rx + 16) == 0x800 && rx[18] == 6 && rx[19] == 4 &&
          be32(rx + 28) == candidate && !equal(rx + 22, mac, 6)) {
        unsigned size = dhcp_request(request, 4, identifier, candidate, 0, 0);
        udp(0xffffffff, 67, request, size, broadcast);
        fail("DHCP address conflict; lease declined");
      }
    }
  }
  ip = candidate;
}
static int dhcp_network(void) {
  static u8 request[512] ALIGNED;
  static const u8 broadcast[6] = {255, 255, 255, 255, 255, 255};
  struct dhcp_reply offer, proxy, reply;
  for (unsigned attempt = 0; attempt < 3; attempt++) {
    memset(&offer, 0, sizeof(offer));
    memset(&proxy, 0, sizeof(proxy));
    ip = 0;
    lease_seconds = 0;
    xid = (u32)now() ^ 0x44484350 ^ attempt;
    source_port = 68;
    unsigned size = dhcp_request(request, 1, 0, 0, 0, 0);
    u64 start, discovering = now();
    u16 discover_seconds = 0;
    for (unsigned discovery = 0; discovery < 3; discovery++) {
      if (discovery) print("DHCP boot information incomplete; rediscovering\n");
      discover_seconds = (u16)((u32)((u32)now() - (u32)discovering) / 1000);
      put16(request + 8, discover_seconds);
      udp(0xffffffff, 67, request, size, broadcast);
      start = now();
      unsigned shift = discovery ? discovery : attempt;
      while (!expired(start, (4U << shift) + xid % 3 - 1)) {
        if (!dhcp_receive(67, &reply) || reply.message != 2) continue;
        if (!reply.address && reply.pxe && !proxy.identifier) proxy = reply;
        else if (ipv4_usable(reply.address) && !offer.identifier) offer = reply;
      }
      if (!offer.identifier || proxy.identifier || offer.pxe ||
          (offer.has_file && ipv4_usable(offer.boot))) break;
    }
    if (!offer.identifier)
      continue;
    int bound = 0, rejected = 0;
    size = dhcp_request(request, 3, offer.identifier, offer.address, 0, 0);
    put16(request + 8, discover_seconds);
    for (unsigned retry = 0; retry < 3 && !bound && !rejected; retry++) {
      udp(0xffffffff, 67, request, size, broadcast);
      start = now();
      while (!expired(start, (4U << retry) + xid % 3 - 1)) {
        if (!dhcp_receive(67, &reply))
          continue;
        if (reply.message == 2 && !reply.address && reply.pxe) {
          if (!proxy.identifier)
            proxy = reply;
          continue;
        }
        if (reply.identifier != offer.identifier)
          continue;
        if (reply.message == 6) {
          rejected = 1;
          break;
        }
        if (reply.message != 5 || reply.address != offer.address ||
            !reply.lease)
          continue;
        if (!reply.mask)
          reply.mask = offer.mask;
        if (!reply.mask || ((~reply.mask + 1) & ~reply.mask))
          continue;
        address_server = reply.identifier;
        ip = reply.address;
        netmask = reply.mask;
        gateway = reply.router;
        if (!gateway)
          gateway = offer.router;
        lease_seconds = reply.lease;
        lease_started = now();
        bound = 1;
        break;
      }
    }
    if (!bound)
      continue;
    dhcp_claim(request, offer.identifier);
    if (!proxy.identifier && reply.pxe)
      proxy = reply;
    server = reply.boot;
    memcpy(bootfile, reply.filename, sizeof(bootfile));
    if (proxy.identifier) {
      print("ProxyDHCP server=");
      hex(proxy.identifier);
      print("\n");
      if ((proxy.control & 8) && proxy.has_file && ipv4_usable(proxy.boot)) {
        server = proxy.boot;
        memcpy(bootfile, proxy.filename, sizeof(bootfile));
      } else {
        u32 destination = proxy.discovery ? proxy.discovery : proxy.identifier;
        server = destination;
        resolve();
        xid++;
        size = dhcp_request(request, 3, destination, 0, proxy.boot_type, 1);
        int found = 0;
        for (unsigned retry = 0; retry < 3 && !found; retry++) {
          udp(destination, 4011, request, size, peer_mac);
          start = now();
          while (!expired(start, 3)) {
            struct dhcp_reply boot;
            if (dhcp_receive(4011, &boot) && boot.message == 5 &&
                boot.identifier == destination &&
                be32(rx + 26) == destination && boot.has_file &&
                ipv4_usable(boot.boot)) {
              server = boot.boot;
              memcpy(bootfile, boot.filename, sizeof(bootfile));
              found = 1;
              break;
            }
          }
        }
        if (!found)
          fail("ProxyDHCP boot service timed out");
      }
    } else if (!reply.has_file || !ipv4_usable(server)) {
      fail("DHCP supplied an address but no boot server or ProxyDHCP offer");
    }
    print("DHCP client=");
    hex(ip);
    print(" server=");
    hex(server);
    print("\n");
    source_port = 20000;
    resolve();
    return 1;
  }
  ip = 0;
  return 0;
}
