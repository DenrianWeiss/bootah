# Imported system icons

The GUI uses these marks only to identify the operating system found in an imported disc. They do not imply sponsorship or endorsement. The SVG sources are kept in `resources/system-icons/`; `scripts/generate_system_icons.py` renders dark and light monochrome PNG variants for offline use. The GUI switches variants with the system theme.

| Icons | Source | Terms |
| --- | --- | --- |
| Ubuntu, Debian, Fedora, Arch Linux, Linux Mint, NixOS, Alpine, openSUSE, CentOS, Red Hat, Rocky Linux, AlmaLinux, Linux (Tux), FreeBSD, NetBSD, OpenBSD | [Simple Icons, commit 90646e4](https://github.com/simple-icons/simple-icons/tree/90646e4bf7f3b89df6c37e65baf3e7bc759a517c/icons) | Project CC0 dedication in `system-icons-simple-icons.txt`; individual marks may remain protected as trademarks. |
| Windows 11 and older Windows glyphs | [Devicon, commit 4fbcc68](https://github.com/devicons/devicon/tree/4fbcc685c9a85434bc01b1c236c9076f54806236/icons) | MIT text in `system-icons-devicon.txt`; Microsoft marks may remain protected as trademarks. |
| Windows XP/7 and Server 2003/2008 identifiers | Original simple four-pane flag geometry drawn for Bootah in `windowsxp.svg`, `windows7.svg`, `windowsserver2003.svg`, and `windowsserver2008.svg` | Bootah project artwork; used only to identify the related Windows generation. Microsoft product names and marks remain Microsoft's trademarks. |
| AOSC OS | [Official AOSC logo, commit 4fa52e8](https://github.com/AOSC-Dev/LOGO/blob/4fa52e8c97c0e8fbcd2896c5ac6f7b31a694fc65/aosc-os.svg) | Official terms in `system-icons-aosc.txt` allow truthful reference to AOSC and its projects. |
| Solaris | [Wikimedia Commons: Solaris OS logo.svg](https://commons.wikimedia.org/wiki/File:Solaris_OS_logo.svg) | Commons labels the geometric/text logo public domain and trademarked. |

Kylin and UOS use the generic Linux penguin until a suitable redistributable logo is documented. Unknown BSD and other systems use the client's neutral disc symbol.
