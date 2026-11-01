#include <assert.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
typedef uint64_t u64;
typedef uint32_t u32;
typedef uint16_t u16;
typedef uint8_t u8;
static u32 xid=0x12345678, ip=0xc0000264, server=0xc0000201,
           netmask=0xffffff00, gateway=0xc00002fe, lease_seconds=3600,
           address_server=0xc0000202;
static u8 mac[6]={8,0,32,1,2,3};
static char bootfile[241]="bootah-obp/grub.img";
static size_t length(const char *s) { return strlen(s); }
static int equal(const void *a,const void *b,size_t n) { return !memcmp(a,b,n); }
static u16 be16(const u8 *p) { return (u16)p[0]*256+p[1]; }
static u32 be32(const u8 *p) { return (u32)be16(p)*65536+be16(p+2); }
static void put16(u8 *p,u16 n) { p[0]=n>>8;p[1]=n; }
static void put32(u8 *p,u32 n) { put16(p,n>>16);put16(p+2,n); }
#include "handoff.h"
#include "native.h"
int main(int argc, char **argv) {
  FILE *f=fopen("../../assets/grub-sparc64.img","rb"); assert(f);
  u8 *image=malloc(GRUB_FILE_LIMIT); assert(image);
  size_t size=fread(image,1,GRUB_FILE_LIMIT,f);fclose(f);
  assert(grub_image(image,size));
  put32(image+20,0x1000000);assert(!grub_image(image,size));free(image);
  assert(safe_bootfile(bootfile));
  const char *bad[]={"bootah-obp.bin","alias/C0000264.SUN4U","../grub.img","x/../grub.img","x//grub.img","tftp:grub.img",""};
  for (unsigned i=0;i<sizeof(bad)/sizeof(*bad);i++) assert(!safe_bootfile(bad[i]));
  u8 packet[512];unsigned n=handoff_packet(packet);
  assert(n<=sizeof(packet) && packet[n-1]==255);
  assert(be32(packet+16)==ip && be32(packet+20)==server);
  assert(!strcmp((char *)packet+108,bootfile));
  assert(!memcmp(packet+28,mac,6));
  unsigned fields=0;
  for(unsigned at=240;packet[at]!=255;) {
    unsigned tag=packet[at++], len=packet[at++];assert(at+len<n);
    if(tag==1) { assert(be32(packet+at)==netmask);fields|=1; }
    if(tag==3) { assert(be32(packet+at)==gateway);fields|=2; }
    if(tag==54) { assert(be32(packet+at)==address_server);fields|=4; }
    if(tag==67) { assert(len==strlen(bootfile));assert(!memcmp(packet+at,bootfile,len));fields|=8; }
    at+=len;
  }
  assert(fields==15);
  char path[256], args[512];
  assert(handoff_path(path,sizeof(path),"/pci/network@1",0));
  assert(!strcmp(path,"/pci/network@1:192.0.2.1,bootah-obp/grub.img"));
  assert(handoff_path(path,sizeof(path),"/pci/network@1",1));
  assert(strstr(path,",192.0.2.100,192.0.2.254,5,5,255.255.255.0,512"));
  assert(!handoff_path(path,8,"/pci/network@1",0));
  const char plan[]="BOOTAH-OBP/1\nloader=ofwboot.net\nbootargs=bsd.rd\n";
  assert(native_plan((const u8 *)plan,sizeof(plan)-1,args));
  assert(!strcmp(args,"bsd.rd") && !strcmp(bootfile,"ofwboot.net"));
  assert(!native_plan((const u8 *)plan,sizeof(plan)-2,args));
  const char recursive[]="BOOTAH-OBP/1\nloader=bootah-obp.cfg\nbootargs=netbsd\n";
  assert(!native_plan((const u8 *)recursive,sizeof(recursive)-1,args));
  for(int i=1;i<argc;i++) {
    f=fopen(argv[i],"rb");assert(f);image=malloc(GRUB_FILE_LIMIT);assert(image);
    size=fread(image,1,GRUB_FILE_LIMIT,f);fclose(f);
    assert(native_image(image,size)==0x100000);
    assert(!native_image(image,64));
    image[24]=255;assert(!native_image(image,size));free(image);
  }
  puts("GRUB/native validation, loop rejection and firmware network context passed");
}
