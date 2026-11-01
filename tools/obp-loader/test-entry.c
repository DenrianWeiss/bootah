#define loader_main network_loader_main
#include "loader.c"
#undef loader_main
extern u8 probe_start[], probe_end[];
void loader_main(u64 entry, u64 stack) {
  cif=(void *)entry; original_stack=stack;
  chosen=find("/chosen"); output=handle("stdout");
  print("Bootah trampoline CPU control\r\n");
  static u8 bytes[]={0,0x80,1,2,3,0xff};
  nodeprop(chosen,"bootargs","bsd.rd",7);
  if (!forthprop(chosen,"bootah-test-binary",bytes,sizeof(bytes)) ||
      !property_matches(chosen,"bootah-test-binary",bytes,sizeof(bytes))) fail("property control failed");
  memset(bytes,0xff,sizeof(bytes));
  u8 checked[6];
  if (property(chosen,"bootah-test-binary",checked,6)!=6 || checked[0]!=0 || checked[5]!=0xff) fail("property value not copied");
  print("Bootah property CIF copy passed\r\n");
  claim(0x4000,GRUB_RESERVATION);
  u64 *a=(void *)0x5dc100;
  a[0]=0x5dc200; a[1]=3; a[2]=1; a[3]=output; a[4]=0x5dc210; a[5]=29; a[6]=0;
  memcpy((void *)0x5dc200,"write",6);
  memcpy((void *)0x5dc210,"Bootah trampoline CIF passed\r\n",30);
  a[8]=0x5dc240; a[9]=0; a[10]=0;memcpy((void *)0x5dc240,"exit",5);
  u64 *get=(void *)0x5dc180;
  get[0]=0x5dc250;get[1]=4;get[2]=1;get[3]=chosen;get[4]=0x5dc260;get[5]=0x5dc300;get[6]=6;get[7]=(u64)-1;
  memcpy((void *)0x5dc250,"getprop",8);
  memcpy((void *)0x5dc260,"bootah-test-binary",19);
  memcpy((void *)0x100000,probe_start,probe_end-probe_start);
  sync_code(0x100000,probe_end-probe_start);
  enter_stage(0x100000,entry);
}
__asm__(".text\n.balign 8\n.global probe_start,probe_end\nprobe_start:\n"
 "save %sp,-192,%sp\n mov %i4,%l0\n sethi %hi(0x5dc180),%l1\n or %l1,%lo(0x5dc180),%l1\n"
 "mov %l1,%o0\n jmpl %l0,%o7\n nop\n ldx [%l1+56],%l2\n cmp %l2,6\n bne 1f\n nop\n"
 "sethi %hi(0x5dc300),%l2\n or %l2,%lo(0x5dc300),%l2\n ldub [%l2],%l3\n brnz %l3,1f\n nop\n"
 "ldub [%l2+5],%l3\n cmp %l3,255\n bne 1f\n nop\n"
 "sethi %hi(0x5dc100),%l1\n or %l1,%lo(0x5dc100),%l1\n"
 "mov %l1,%o0\n jmpl %l0,%o7\n nop\n add %l1,64,%o0\n jmpl %l0,%o7\n nop\n1: ba 1b\n nop\nprobe_end:\n");
