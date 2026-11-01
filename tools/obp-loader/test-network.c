/* Linux-only CIF double: exercise the unchanged protocol/state code over Bootah's wire. */
#define _GNU_SOURCE
#include <arpa/inet.h>
#include <poll.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/mman.h>
#include <sys/socket.h>
#include <time.h>
#include <unistd.h>
#define OBP_HOST_TEST
#include "loader.c"
static int wire, external, drop_proxy;
static unsigned discovers, dropped;
static u8 pending[2048];
static unsigned pending_size;
static const char canonical[] = "/test/network@0";
struct prop { u64 node; char key[32]; unsigned size; u8 bytes[1024]; };
static struct prop props[8];
static unsigned properties;
static void transfer(int fd, void *buffer, unsigned size, int writing) {
  u8 *p = buffer;
  while (size) {
    long n = writing ? write(fd, p, size) : read(fd, p, size);
    if (n <= 0) { perror("wire"); exit(2); }
    p += n; size -= n;
  }
}
static void external_lease(const u8 *f, unsigned size) {
  if (!external || size < 282 || be16(f + 12) != 0x800 || be16(f + 36) != 67) return;
  const u8 *req = f + 42;
  u8 message[1], selected[4];
  if (dhcp_option(req, size - 42, 53, message, 1) != 1 || (message[0] != 1 && message[0] != 3)) return;
  if (message[0] == 3 && (dhcp_option(req, size - 42, 54, selected, 4) != 4 || be32(selected) != 0xc000020a)) return;
  memset(pending, 0, sizeof(pending));
  memset(pending, 255, 6); memcpy(pending + 6, "\2\0\0\0\0\12", 6);
  put16(pending + 12, 0x800); pending[14] = 0x45; pending[22] = 64; pending[23] = 17;
  put32(pending + 26, 0xc000020a); put32(pending + 30, 0xffffffff);
  put16(pending + 34, 67); put16(pending + 36, 68);
  u8 *b = pending + 42;
  b[0] = 2; b[1] = 1; b[2] = 6;
  memcpy(b + 4, req + 4, 4); memcpy(b + 28, req + 28, 6);
  put32(b + 16, 0xc0000278); put32(b + 236, 0x63825363);
  u8 options[] = {53,1,message[0] == 1 ? 2 : 5,54,4,192,0,2,10,1,4,255,255,255,0,51,4,0,0,14,16,255};
  memcpy(b + 240, options, sizeof(options));
  put16(pending + 16, 328); put16(pending + 38, 308);
  put16(pending + 24, checksum(pending + 14, 20)); pending_size = 342;
}
static int service(void *argument) {
  u64 *a = argument;
  const char *name = (void *)a[0];
  if (!strcmp(name, "finddevice")) {
    const char *p = (void *)a[3]; a[4] = !strcmp(p,"/chosen") ? 1 : !strcmp(p,"/options") ? 3 : 2; return 0;
  }
  if (!strcmp(name, "getprop")) {
    const char *key = (void *)a[4];
    const void *bytes = NULL; unsigned size = 0; u8 handle_bytes[4] = {0,0,0,1};
    for (unsigned i = 0; i < properties; i++)
      if (props[i].node == a[3] && !strcmp(props[i].key,key)) { bytes=props[i].bytes;size=props[i].size; }
    if (!bytes && !strcmp(key,"stdout")) { bytes=handle_bytes;size=4; }
    if (!bytes && !strcmp(key,"bootpath")) { bytes="net";size=4; }
    if (!bytes && !strcmp(key,"device_type")) { bytes="network";size=8; }
    if (!bytes && !strcmp(key,"mac-address")) { bytes="\122\124\0\22\64\126";size=6; }
    if (!bytes) { a[7]=(u64)-1;return 0; }
    if (size > a[6]) size = a[6]; memcpy((void *)a[5],bytes,size);a[7]=size;return 0;
  }
  if (!strcmp(name,"setprop")) {
    unsigned i;
    for (i=0;i<properties;i++) if (props[i].node==a[3] && !strcmp(props[i].key,(void *)a[4])) break;
    if (i==properties) properties++;
    if (properties>8 || a[6]>1024) exit(2);
    props[i].node=a[3];strcpy(props[i].key,(void *)a[4]);props[i].size=a[6];memcpy(props[i].bytes,(void *)a[5],a[6]);a[7]=a[6];return 0;
  }
  if (!strcmp(name,"canon")) { memcpy((void *)a[4],canonical,sizeof(canonical));a[6]=sizeof(canonical)-1;return 0; }
  if (!strcmp(name,"open")) { a[4]=7;return 0; }
  if (!strcmp(name,"close")) return 0;
  if (!strcmp(name,"milliseconds")) { struct timespec t;clock_gettime(CLOCK_MONOTONIC,&t);a[3]=(u32)((u64)t.tv_sec*1000+t.tv_nsec/1000000);return 0; }
  if (!strcmp(name,"claim")) {
    void *p=mmap((void *)a[3],a[4],PROT_READ|PROT_WRITE|PROT_EXEC,MAP_PRIVATE|MAP_ANONYMOUS|MAP_FIXED,-1,0);
    a[6]=(u64)p;return 0;
  }
  if (!strcmp(name,"write")) {
    if (a[3]==1) fwrite((void *)a[4],1,a[5],stdout);
    else {
      const u8 *f=(void *)a[4];u8 type;
      if (a[5]>=282 && be16(f+12)==0x800 && be16(f+36)==67 && dhcp_option(f+42,a[5]-42,53,&type,1)==1 && type==1) discovers++;
      external_lease((void *)a[4],a[5]);u32 n=htonl(a[5]);transfer(wire,&n,4,1);transfer(wire,(void *)a[4],a[5],1); }
    a[6]=a[5];return 0;
  }
  if (!strcmp(name,"read")) {
    if (pending_size) { memcpy((void *)a[4],pending,pending_size);a[6]=pending_size;pending_size=0;return 0; }
    struct pollfd p={wire,POLLIN,0};
    if (poll(&p,1,5)<=0) { a[6]=(u64)-2;return 0; }
    u32 size;transfer(wire,&size,4,0);size=ntohl(size);
    if (size>a[5]) exit(2);transfer(wire,(void *)a[4],size,0);
    const u8 *f=(void *)a[4];u8 type;
    if (drop_proxy && size>=282 && be16(f+12)==0x800 && be16(f+34)==67 && be32(f+58)==0 &&
        dhcp_option(f+42,size-42,53,&type,1)==1 && type==2) {
      drop_proxy--;dropped++;a[6]=(u64)-2;return 0;
    }
    a[6]=size;return 0;
  }
  if (!strcmp(name,"exit")) exit(1);
  fprintf(stderr,"unknown CIF service %s\n",name);exit(2);
}
void test_enter_stage(u64 target, u64 firmware) {
  (void)firmware;
  printf("TEST discovers=%u proxy_offers_dropped=%u\n",discovers,dropped);
  printf("TEST entry=%lx server=%08x client=%08x bootfile=%s bootargs=%s\n",target,server,ip,bootfile,bootargs);
  for (unsigned i=0;i<properties;i++) {
    printf("PROP %s=",props[i].key);
    for(unsigned j=0;j<props[i].size;j++) printf("%02x",props[i].bytes[j]);
    putchar('\n');
  }
  if (target==GRUB_ADDRESS && be32((void *)target+4)!=0x9010000c) exit(2);
  fflush(stdout);exit(0);
}
int main(int argc,char **argv) {
  if (argc<3) return 2;
  external=argc>3;
  const char *loss=getenv("BOOTAH_TEST_DROP_PROXY");drop_proxy=loss?atoi(loss):0;
  wire=socket(AF_INET,SOCK_STREAM,0);
  struct sockaddr_in to={.sin_family=AF_INET,.sin_port=htons(atoi(argv[2]))};
  if (inet_pton(AF_INET,argv[1],&to.sin_addr)!=1 || connect(wire,(void *)&to,sizeof(to))) { perror("connect");return 2; }
  loader_main((u64)service,0x7fffffff0000UL);
}
