#include <arpa/inet.h>
#include <netinet/tcp.h>
#include <sys/socket.h>
#include <sys/time.h>
#include <poll.h>
#include <unistd.h>
#include <stdlib.h>
#include <string.h>
#include <stdint.h>

struct pcap_if {struct pcap_if *next;char *name;char *description;void *addresses;unsigned flags;};
struct pcap_header {struct timeval time;unsigned caplen,len;};
struct capture {int fd;unsigned char input[131080],packet[65536];size_t used;struct pcap_header header;};
int pcap_findalldevs(struct pcap_if **result,char *error){static struct pcap_if device={0,"netboot","Netboot isolated emulator network",0,0};*result=&device;return 0;}
void *pcap_open_live(const char *name,int snaplen,int promisc,int timeout,char *error){
    int fd=socket(AF_INET,SOCK_STREAM,0);struct sockaddr_in server={.sin_family=AF_INET,.sin_port=htons(19090)};inet_pton(AF_INET,"127.0.0.1",&server.sin_addr);
    if(connect(fd,(void*)&server,sizeof(server))){strcpy(error,"Netboot wire unavailable");close(fd);return 0;}
    int enabled=1;setsockopt(fd,IPPROTO_TCP,TCP_NODELAY,&enabled,sizeof(enabled));
    struct capture *c=calloc(1,sizeof(*c));c->fd=fd;return c;
}
int pcap_setnonblock(void *capture,int enabled,char *error){return 0;}
int pcap_compile(void *capture,void *program,const char *filter,int optimize,unsigned mask){return 0;}
int pcap_setfilter(void *capture,void *program){return 0;}
const char *pcap_geterr(void *capture){return "Netboot wire error";}
static int write_all(int fd,const void *data,size_t size){const unsigned char *p=data;while(size){ssize_t n=send(fd,p,size,0);if(n<=0)return -1;p+=n;size-=n;}return 0;}
int pcap_sendpacket(void *capture,const unsigned char *packet,int size){struct capture *c=capture;uint32_t length=htonl(size);if(size<0||size>65536)return -1;return write_all(c->fd,&length,4)||write_all(c->fd,packet,size)?-1:0;}
int pcap_next_ex(void *capture,struct pcap_header **header,const unsigned char **packet){
    struct capture *c=capture;
    if(c->used<4){struct pollfd p={c->fd,POLLIN,0};if(poll(&p,1,0)<=0)return 0;}
    for(;;){
        if(c->used>=4){uint32_t size;memcpy(&size,c->input,4);size=ntohl(size);if(!size||size>65536)return -1;
            if(c->used>=size+4){memcpy(c->packet,c->input+4,size);memmove(c->input,c->input+4+size,c->used-4-size);c->used-=4+size;gettimeofday(&c->header.time,0);c->header.caplen=c->header.len=size;*header=&c->header;*packet=c->packet;return 1;}}
        ssize_t n=recv(c->fd,c->input+c->used,sizeof(c->input)-c->used,MSG_DONTWAIT);if(n<0)return 0;if(!n)return -1;c->used+=n;
    }
}
void pcap_close(void *capture){struct capture *c=capture;if(c){close(c->fd);free(c);}}
