#include <assert.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#define OBP_HOST_TEST
#include "loader.c"
static u8 saved[1024];
static unsigned saved_size, sets, definitions, copies;
static char saved_name[64];
static int sun_string;
static int service(void *arg) {
  u64 *a=arg;const char *op=(void *)a[0];
  if (!strcmp(op,"getprop")) {
    const char *key=(void *)a[4];
    if (!strcmp(key,"mac-address")) { memcpy((void *)a[5],"\0\24\117\363\325\350",6);a[7]=6; }
    else if (!strcmp(key,saved_name)) { assert(a[6]>=saved_size);memcpy((void *)a[5],saved,saved_size);a[7]=saved_size; }
    else a[7]=(u64)-1;
    return 0;
  }
  if (!strcmp(op,"setprop")) {
    sets++;a[7]=(u64)-1;
    if (!strcmp((void *)a[4],"bootargs")) {
      strcpy(saved_name,(void *)a[4]);saved_size=a[6];memcpy(saved,(void *)a[5],saved_size);
      a[7]=saved_size-1; /* Accept a firmware return-length quirk only after exact readback. */
    }
    return 0;
  }
  if (!strcmp(op,"interpret")) {
    if (a[1]==4) {
      assert(a[2]==2 && a[4]==1);a[7]=0;a[8]=sun_string;
      if (sun_string) {
        assert(!strcmp((void *)a[3],"bootah-obp-args"));
        strcpy(saved_name,"bootargs");saved_size=a[5];memcpy(saved,(void *)a[6],saved_size);
      }
    }
    else if (a[1]==1) {
      if (strstr((void *)a[3],"encode-bytes")) definitions++;
      else assert(strstr((void *)a[3],"args-buf"));
      a[4]=0;
    }
    else {
      assert(a[1]==6 && a[2]==1 && a[4]==1);
      assert(!strcmp((void *)a[3],"bootah-obp-set-property"));
      assert(a[5]==strlen((void *)a[6]));strcpy(saved_name,(void *)a[6]);saved_size=a[7];
      memcpy(saved,(void *)a[8],saved_size);copies++;a[9]=0;
    }
    return 0;
  }
  abort();
}
void test_enter_stage(u64 target,u64 firmware) { (void)target;(void)firmware;abort(); }
int main(void) {
  cif=service;chosen=1;
  const u8 address[]={0,0x14,0x4f,0xf3,0xd5,0xe8};
  nodeprop(2,"mac-address",address,6);assert(!sets && !copies);
  nodeprop(1,"bootargs","bsd.rd",7);assert(sets==1 && !copies);
  sun_string=1;
  nodeprop(1,"bootargs","netbsd",7);assert(sets==1 && !copies);
  assert(!strcmp((void *)saved,"netbsd"));
  u8 packet[]={2,1,6,0,0x80,0xff};
  nodeprop(1,"bootp-response",packet,sizeof(packet));assert(copies==1 && definitions==1);
  memset(packet,0,sizeof(packet));assert(saved[4]==0x80 && saved[5]==0xff);
  nodeprop(1,"dhcp-response",saved,saved_size);assert(copies==2 && definitions==1);
  u8 request[512];
  dhcp_request(request,1,0,0,0,0);assert(be16(request+10)==0x8000);
  dhcp_request(request,3,0xc0000201,0xc0000264,0,0);assert(be16(request+10)==0x8000);
  dhcp_request(request,3,0xc0000201,0,0,1);assert(be16(request+10)==0);
  puts("Readonly MAC, verified length quirk and firmware-owned binary properties passed");
  return 0;
}
