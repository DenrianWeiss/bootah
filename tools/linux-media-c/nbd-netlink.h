/* SPDX-License-Identifier: MIT
 * Wire constants from Linux's published NBD generic-netlink UAPI.
 * Kept locally so old installer sysroots can build the modern negotiation path.
 */
#ifndef BOOTAH_NBD_NETLINK_H
#define BOOTAH_NBD_NETLINK_H
#if __has_include(<linux/nbd-netlink.h>)
#include <linux/nbd-netlink.h>
#else
enum { NBD_CMD_CONNECT=1, NBD_CMD_DISCONNECT=2 };
enum { NBD_ATTR_INDEX=1, NBD_ATTR_SIZE_BYTES=2, NBD_ATTR_BLOCK_SIZE_BYTES=3,
       NBD_ATTR_SERVER_FLAGS=5, NBD_ATTR_CLIENT_FLAGS=6, NBD_ATTR_SOCKETS=7 };
enum { NBD_SOCK_ITEM=1, NBD_SOCK_FD=1 };
#endif
#endif
