namespace Netboot;
internal static class AccessGuidance {
    public static string? DownloadUrl(string message) => message.Contains("https://npcap.com/#download",StringComparison.Ordinal)?"https://npcap.com/#download":message.Contains("https://www.wireshark.org/download.html",StringComparison.Ordinal)?"https://www.wireshark.org/download.html":null;
    public static string Describe(string message) {
        foreach(var (oldText,newText) in new[]{
            ("Network access was not authorized. Retry and approve the system authorization prompt; on Linux ensure a desktop polkit authentication agent is running.","Network access was not authorized. Check again to retry."),
            ("Network permission is required. Use Authorize detection for DHCP probing. To start services on Linux, install the Bootah network helper and approve polkit or sudo authorization.","Network access is required. Recheck to request authorization."),
            ("Grant packet capture access by installing ChmodBPF from the official Wireshark macOS disk image: https://www.wireshark.org/download.html . Restart Bootah after installation. Authorize detection only grants access to a temporary DHCP probe.","Install ChmodBPF from https://www.wireshark.org/download.html and restart Bootah."),
            ("Open Network permissions in Bootah and enable its bundled network helper in macOS System Settings. Alternatively, install ChmodBPF from the official Wireshark macOS disk image: https://www.wireshark.org/download.html . Then retry.","Enable the Bootah network helper in macOS System Settings, or install ChmodBPF from https://www.wireshark.org/download.html ."),
            ("Network permission checks could not complete. Check the core connection and retry from network settings.","Check failed. Check the core connection and retry.")
        })message=message.Replace(oldText,Strings.Tr(newText),StringComparison.Ordinal);
        message=message.Replace("Address already in use",Strings.Tr("Address already in use"),StringComparison.Ordinal);
        foreach(var key in new string[] {
            "Start services and approve the macOS administrator prompt to allow the required network ports. Packet capture access does not grant permission to bind these ports.",
            "Enable the Bootah network helper in macOS System Settings to allow the required network ports.",
            "Administrator authorization was cancelled. Services remain stopped.",
            "Administrator authorization failed. Retry starting services.",
            "Administrator authorization timed out. Retry starting services.",
            "Network port access was denied. Check for another service using an exclusive bind, reserved ports, or system security restrictions. Installing Npcap does not grant access to this port.",
            "Network privileges are required. Start services and approve the polkit or sudo prompt.",
            "The configured network services require Root access. In Bootah settings, stop the local core, choose Root startup, and approve access in your Root manager. If this device does not provide Root, use a desktop host for standard network boot services.",
            "Network permission checks could not complete. Check the core connection and retry from network settings.",

            "Open Network permissions in Bootah and enable its bundled network helper in macOS System Settings. Alternatively, install ChmodBPF from the official Wireshark macOS disk image: https://www.wireshark.org/download.html . Then retry.",
            "Network capture requires BPF access. Install ChmodBPF from the official Wireshark macOS disk image: https://www.wireshark.org/download.html . Then restart Bootah.",

            "The system libpcap library could not be loaded. Restore the system library and restart Bootah.",
            "Install Npcap from https://npcap.com/#download with WinPcap API-compatible mode enabled, then restart Bootah.",
            "Allow the Npcap authorization prompt. If access is still denied, restart Bootah using Run as administrator.",
            "Grant packet capture access by installing ChmodBPF from the official Wireshark macOS disk image: https://www.wireshark.org/download.html . Restart Bootah after installation. Authorize detection only grants access to a temporary DHCP probe.",
            "Install the Bootah network helper using sudo sh scripts/install_linux_core.sh /path/to/netboot-core from the Bootah distribution.",
            "Install polkit (pkexec) and a desktop authentication agent using your distribution package manager, then retry and approve the authorization prompt. For TUI, install sudo and use --authorization sudo.",
            "Network access was not authorized. Retry and approve the system authorization prompt; on Linux ensure a desktop polkit authentication agent is running.",
            "Network permission is required. Use Authorize detection for DHCP probing. To start services on Linux, install the Bootah network helper and approve polkit or sudo authorization.",
            "Permission to bind this network port was denied. Grant network access to the Bootah core on the service host and retry. On Windows, check administrator and security software restrictions; on macOS, ask an administrator to grant the core the required service permissions.",
            "The Bootah core is missing or cannot be executed. Download and reinstall the complete Bootah package for this platform, or correct NETBOOT_CORE and its execute permissions.",
        }) message=message.Replace(key,Strings.Tr(key),StringComparison.Ordinal);
        return string.Join("\n",message.Split('\n').Select(Strings.Tr));
    }
}
