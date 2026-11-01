/* SPDX-License-Identifier: MIT */
#define _GNU_SOURCE
#include <arpa/inet.h>
#include <errno.h>
#include <linux/genetlink.h>
#include "nbd-netlink.h"
#include <linux/nbd.h>
#include <linux/fs.h>
#include <fcntl.h>
#include <limits.h>
#include <signal.h>
#include <sys/ioctl.h>
#include <sys/wait.h>
#include <linux/netlink.h>
#include <netinet/tcp.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/socket.h>
#include <sys/time.h>
#include <unistd.h>

static void die(const char *what) { fprintf(stderr,"bootah-media: %s: %s\n",what,strerror(errno)); exit(1); }
static void check(int ok, const char *what) { if (!ok) { errno=EPROTO; die(what); } }
static void transfer(int fd, void *buf, size_t n, int writing) {
    unsigned char *p=buf;
    while(n) {
        ssize_t k=writing ? send(fd,p,n,MSG_NOSIGNAL) : recv(fd,p,n,0);
        if(k<0 && errno==EINTR) continue;
        if(k<=0) { if(!k) errno=ECONNRESET; die("NBD handshake"); }
        p+=k; n-=k;
    }
}
static uint64_t be64(const unsigned char *p) {
    uint64_t n=0; for(unsigned i=0;i<8;i++) n=(n<<8)|p[i]; return n;
}
struct packet { unsigned char b[8192]; size_t n; int unavailable; };
static void attr(struct packet *p, uint16_t type, const void *value, size_t n) {
    size_t size=NLA_ALIGN(n+NLA_HDRLEN);
    check(p->n+size<=sizeof(p->b),"netlink attribute too large");
    struct nlattr a={.nla_len=n+NLA_HDRLEN,.nla_type=type};
    memcpy(p->b+p->n,&a,sizeof(a)); memcpy(p->b+p->n+NLA_HDRLEN,value,n);
    memset(p->b+p->n+NLA_HDRLEN+n,0,size-NLA_HDRLEN-n); p->n+=size;
}
static uint64_t find(struct packet *p, unsigned type, size_t size) {
    size_t off=0;
    while(off+NLA_HDRLEN<=p->n) {
        struct nlattr a; memcpy(&a,p->b+off,sizeof(a));
        check(a.nla_len>=NLA_HDRLEN && off+a.nla_len<=p->n,"invalid netlink attribute");
        if((a.nla_type&NLA_TYPE_MASK)==type) {
            check(a.nla_len==NLA_HDRLEN+size,"wrong netlink attribute size");
            const unsigned char *value=p->b+off+NLA_HDRLEN;
            if(size==2) { uint16_t n; memcpy(&n,value,2); return n; }
            if(size==4) { uint32_t n; memcpy(&n,value,4); return n; }
            if(size==8) { uint64_t n; memcpy(&n,value,8); return n; }
            check(0,"unsupported netlink integer width");
        }
        off+=NLA_ALIGN(a.nla_len);
    }
    errno=EPROTO; die("missing netlink attribute"); return 0;
}
static struct packet request(int fd, uint16_t family, unsigned char cmd, unsigned char version,
                             uint32_t seq, struct packet *attrs, int reply) {
    struct packet out={0}, result={0};
    struct nlmsghdr h={.nlmsg_len=NLMSG_HDRLEN+GENL_HDRLEN+attrs->n,.nlmsg_type=family,
        .nlmsg_flags=NLM_F_REQUEST|NLM_F_ACK,.nlmsg_seq=seq};
    struct genlmsghdr g={.cmd=cmd,.version=version};
    memcpy(out.b,&h,sizeof(h)); memcpy(out.b+NLMSG_HDRLEN,&g,sizeof(g));
    memcpy(out.b+NLMSG_HDRLEN+GENL_HDRLEN,attrs->b,attrs->n);
    struct sockaddr_nl kernel={.nl_family=AF_NETLINK};
    if(sendto(fd,out.b,h.nlmsg_len,0,(void*)&kernel,sizeof(kernel))!=(ssize_t)h.nlmsg_len) die("netlink send");
    int ack=0, got=0;
    for(int tries=0;tries<32;tries++) {
        unsigned char buf[65536]; struct sockaddr_nl sender={0}; socklen_t len=sizeof(sender);
        ssize_t n=recvfrom(fd,buf,sizeof(buf),MSG_TRUNC,(void*)&sender,&len);
        if(n<0) die("netlink receive");
        check(n<=(ssize_t)sizeof(buf) && len==sizeof(sender) && sender.nl_pid==0,"invalid netlink sender");
        size_t off=0;
        while(off+NLMSG_HDRLEN<=(size_t)n) {
            memcpy(&h,buf+off,sizeof(h));
            check(h.nlmsg_len>=NLMSG_HDRLEN && off+h.nlmsg_len<=(size_t)n,"invalid netlink packet");
            if(h.nlmsg_seq==seq) {
                if(h.nlmsg_type==NLMSG_ERROR) {
                    int error; check(h.nlmsg_len>=NLMSG_HDRLEN+sizeof(error),"short netlink status");
                    memcpy(&error,buf+off+NLMSG_HDRLEN,sizeof(error));
                    if(error) {
                        if(family==GENL_ID_CTRL && (error==-ENOENT || error==-EOPNOTSUPP)) { result.unavailable=1; return result; }
                        errno=-error; die("NBD netlink");
                    }
                    ack=1;
                } else if(h.nlmsg_type==family) {
                    check(h.nlmsg_len>=NLMSG_HDRLEN+GENL_HDRLEN,"short netlink reply");
                    result.n=h.nlmsg_len-NLMSG_HDRLEN-GENL_HDRLEN;
                    check(result.n<=sizeof(result.b),"large netlink reply");
                    memcpy(result.b,buf+off+NLMSG_HDRLEN+GENL_HDRLEN,result.n); got=1;
                }
            }
            off+=NLMSG_ALIGN(h.nlmsg_len);
        }
        check(off==(size_t)n,"truncated netlink packet");
        if(ack && (!reply || got)) return result;
    }
    errno=ETIMEDOUT; die("netlink response limit"); return result;
}
static uint32_t decimal(const char *p) {
    check(*p!=0,"empty decimal value"); uint32_t n=0;
    for(;*p;p++) {
        check(*p>='0' && *p<='9' && n<=(UINT32_MAX-(*p-'0'))/10,"invalid decimal value");
        n=n*10+*p-'0';
    }
    return n;
}
static uint64_t sys_value(uint32_t index, const char *attribute) {
    char path[96], text[32]; snprintf(path,sizeof(path),"/sys/block/nbd%u/%s",index,attribute);
    FILE *f=fopen(path,"r"); if(!f) return UINT64_MAX;
    char *read=fgets(text,sizeof(text),f); fclose(f); if(!read) return UINT64_MAX;
    uint64_t n=0; for(char *p=text;*p>='0' && *p<='9';p++) n=n*10+*p-'0';
    return n;
}
static void worker(int dev, int status) {
    signal(SIGPIPE,SIG_IGN);
    int result=ioctl(dev,NBD_DO_IT,0), error=result<0 ? errno : 0;
    (void)write(status,&error,sizeof(error));
    (void)ioctl(dev,NBD_CLEAR_SOCK,0); _exit(error ? 1 : 0);
}
static int legacy(int tcp, uint64_t size, uint32_t index, const char *device) {
    check(size/512<=(uint64_t)ULONG_MAX,"export exceeds legacy userspace ABI size");
    int dev=open(device,O_RDWR|O_CLOEXEC); if(dev<0) die("open legacy NBD device");
    if(ioctl(dev,NBD_SET_BLKSIZE,512UL)<0 || ioctl(dev,NBD_SET_SIZE_BLOCKS,(unsigned long)(size/512))<0)
        die("legacy NBD size");
    int ro=1;
    if(ioctl(dev,BLKROSET,&ro)<0) die("legacy NBD read-only");
    if(ioctl(dev,NBD_SET_SOCK,(unsigned long)tcp)<0) die("legacy NBD socket");
    int status[2]; if(pipe(status)<0) { ioctl(dev,NBD_CLEAR_SOCK,0); die("legacy NBD status pipe"); }
    pid_t pid=fork();
    if(pid<0) { ioctl(dev,NBD_CLEAR_SOCK,0); die("legacy NBD fork"); }
    if(!pid) {
        close(status[0]); setsid();
        int null=open("/dev/null",O_RDWR);
        if(null>=0) { for(int i=0;i<3;i++) dup2(null,i); if(null>2) close(null); }
        if(!access("/etc/initrd-release",F_OK)) {
            int fds[]={dev,tcp,status[1]};
            for(unsigned i=0;i<3;i++) if(fcntl(fds[i],F_SETFD,0)<0) {
                int error=errno; (void)write(status[1],&error,sizeof(error)); _exit(1);
            }
            char a[16],b[16]; snprintf(a,sizeof(a),"%d",dev); snprintf(b,sizeof(b),"%d",status[1]);
            char *args[]={"@bootah-media","--nbd-worker",a,b,NULL};
            execv("/proc/self/exe",args);
            int error=errno; (void)write(status[1],&error,sizeof(error)); _exit(1);
        }
        worker(dev,status[1]);
    }
    close(status[1]); if(fcntl(status[0],F_SETFL,O_NONBLOCK)<0) die("legacy NBD pipe flags");
    int failure=ETIMEDOUT;
    for(unsigned tries=0;tries<100;tries++) {
        int error=0; ssize_t count=read(status[0],&error,sizeof(error));
        if(count>0 || count==0) { failure=error ? error : EIO; break; }
        if(errno!=EAGAIN && errno!=EINTR) { failure=errno; break; }
        if(sys_value(index,"size")==size/512 && sys_value(index,"pid")== (uint64_t)pid) {
            close(status[0]); close(dev); close(tcp); return 0;
        }
        usleep(50000);
    }
    (void)ioctl(dev,NBD_DISCONNECT,0); kill(pid,SIGKILL); (void)waitpid(pid,NULL,WNOHANG);
    close(status[0]); close(dev); errno=failure; die("legacy NBD worker did not become ready"); return 1;
}
int main(int argc, char **argv) {
    if(argc==4 && !strcmp(argv[1],"--nbd-worker")) {
        uint32_t dev=decimal(argv[2]), status=decimal(argv[3]);
        check(dev<=INT_MAX && status<=INT_MAX,"invalid worker descriptor");
        worker(dev,status);
    }
    int force_legacy=argc==7 && !strcmp(argv[6],"--legacy-ioctl");
    if((argc!=6 && !force_legacy) || strcmp(argv[1],"nbd")) {
        fprintf(stderr,"usage: bootah-media nbd SERVER PORT EXPORT /dev/nbdN [--legacy-ioctl]\n"); return 1;
    }
    check(!strncmp(argv[5],"/dev/nbd",8) && argv[5][8],"invalid NBD device");
    uint32_t index=decimal(argv[5]+8);
    struct sockaddr_in address={.sin_family=AF_INET};
    uint32_t server=0; const char *p=argv[2];
    for(unsigned i=0;i<4;i++) {
        unsigned octet=0, digits=0;
        while(*p>='0' && *p<='9') { octet=octet*10+*p++-'0'; digits++; check(digits<=3 && octet<=255,"invalid IPv4 address"); }
        check(digits && (i==3 ? *p==0 : *p=='.'),"C media server requires numeric IPv4");
        server=(server<<8)|octet; if(i<3) p++;
    }
    uint32_t port=decimal(argv[3]); check(port && port<=65535,"invalid TCP port");
    address.sin_addr.s_addr=htonl(server); address.sin_port=htons(port);
    int tcp=socket(AF_INET,SOCK_STREAM|SOCK_CLOEXEC,0); if(tcp<0) die("TCP socket");
    struct timeval timeout={.tv_sec=15};
    if(setsockopt(tcp,SOL_SOCKET,SO_RCVTIMEO,&timeout,sizeof(timeout))<0 ||
       setsockopt(tcp,SOL_SOCKET,SO_SNDTIMEO,&timeout,sizeof(timeout))<0) die("TCP timeout");
    if(connect(tcp,(void*)&address,sizeof(address))<0) die("TCP connect");
    int one=1; if(setsockopt(tcp,IPPROTO_TCP,TCP_NODELAY,&one,sizeof(one))<0) die("TCP_NODELAY");
    unsigned char hello[18]; transfer(tcp,hello,sizeof(hello),0);
    check(!memcmp(hello,"NBDMAGICIHAVEOPT",16) && (hello[17]&3)==3,"fixed-newstyle/no-zeroes required");
    uint32_t flags=htonl(3); transfer(tcp,&flags,4,1);
    size_t namelen=strlen(argv[4]); check(namelen && namelen<=4096,"invalid export name");
    unsigned char option[16]={'I','H','A','V','E','O','P','T',0,0,0,1};
    uint32_t length=htonl(namelen); memcpy(option+12,&length,4);
    transfer(tcp,option,sizeof(option),1); transfer(tcp,argv[4],namelen,1);
    unsigned char info[10]; transfer(tcp,info,sizeof(info),0);
    uint64_t size=be64(info), server_flags=((uint64_t)info[8]<<8)|info[9];
    check(size && !(size%512) && (server_flags&3)==3,"read-only aligned export required");
    timeout.tv_sec=0;
    if(setsockopt(tcp,SOL_SOCKET,SO_RCVTIMEO,&timeout,sizeof(timeout))<0 ||
       setsockopt(tcp,SOL_SOCKET,SO_SNDTIMEO,&timeout,sizeof(timeout))<0) die("clear TCP timeout");
    if(force_legacy) return legacy(tcp,size,index,argv[5]);
    int nl=socket(AF_NETLINK,SOCK_RAW|SOCK_CLOEXEC,NETLINK_GENERIC); if(nl<0) die("netlink socket");
    struct sockaddr_nl local={.nl_family=AF_NETLINK};
    if(bind(nl,(void*)&local,sizeof(local))<0) die("netlink bind");
    timeout.tv_sec=5; if(setsockopt(nl,SOL_SOCKET,SO_RCVTIMEO,&timeout,sizeof(timeout))<0) die("netlink timeout");
    struct packet attrs={0}; attr(&attrs,CTRL_ATTR_FAMILY_NAME,"nbd",4);
    struct packet response=request(nl,GENL_ID_CTRL,CTRL_CMD_GETFAMILY,2,1,&attrs,1);
    if(response.unavailable) {
        close(nl); fprintf(stderr,"bootah-media: kernel lacks NBD netlink; using legacy read-only mapping\n");
        return legacy(tcp,size,index,argv[5]);
    }
    uint16_t family=find(&response,CTRL_ATTR_FAMILY_ID,2);
    attrs.n=0; uint64_t block=512, client_flags=0; uint32_t sock=tcp;
    attr(&attrs,NBD_ATTR_INDEX,&index,4); attr(&attrs,NBD_ATTR_SIZE_BYTES,&size,8);
    attr(&attrs,NBD_ATTR_BLOCK_SIZE_BYTES,&block,8); attr(&attrs,NBD_ATTR_SERVER_FLAGS,&server_flags,8);
    attr(&attrs,NBD_ATTR_CLIENT_FLAGS,&client_flags,8);
    struct packet socket_attr={0}, sockets={0}; attr(&socket_attr,NBD_SOCK_FD,&sock,4);
    attr(&sockets,NBD_SOCK_ITEM|NLA_F_NESTED,socket_attr.b,socket_attr.n);
    attr(&attrs,NBD_ATTR_SOCKETS|NLA_F_NESTED,sockets.b,sockets.n);
    response=request(nl,family,NBD_CMD_CONNECT,1,2,&attrs,1);
    check(find(&response,NBD_ATTR_INDEX,4)==index,"unexpected NBD index");
    char path[80]; snprintf(path,sizeof(path),"/sys/block/nbd%u/size",index);
    for(int tries=0;tries<100;tries++) {
        FILE *f=fopen(path,"r"); uint64_t sectors=0; char text[32];
        if(f) {
            char *read=fgets(text,sizeof(text),f); fclose(f);
            if(read) {
                for(char *p=text;*p>='0' && *p<='9';p++) sectors=sectors*10+*p-'0';
                if(sectors==size/512) { close(nl); close(tcp); return 0; }
            }
        }
        usleep(50000);
    }
    attrs.n=0; attr(&attrs,NBD_ATTR_INDEX,&index,4);
    request(nl,family,NBD_CMD_DISCONNECT,1,3,&attrs,0);
    errno=ETIMEDOUT; die("NBD size did not become ready");
}
