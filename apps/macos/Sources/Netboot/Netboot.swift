import SwiftUI
import AppKit
import Foundation
import UniformTypeIdentifiers
import ServiceManagement
import Security

final class NoRedirect: NSObject, URLSessionTaskDelegate, @unchecked Sendable {
    func urlSession(_ session:URLSession, task:URLSessionTask, willPerformHTTPRedirection response:HTTPURLResponse, newRequest request:URLRequest, completionHandler:@escaping @Sendable (URLRequest?) -> Void) { completionHandler(nil) }
}

struct Machine: Codable, Identifiable, Hashable {
    var id = ""
    var mac = ""
    var ip: String?
    var name = ""
    var model = ""
    var architecture = "unknown"
    var `protocol` = ""
    var vendor = ""
    var uuid = ""
    var profileId: String?
    var lastSeen: UInt64 = 0
}
struct SystemImage: Codable, Identifiable, Hashable {
    var id = UUID().uuidString
    var name = ""
    var systemId: String?
    var systemName: String?
    var architecture = "x64"
    var kind = "linux"
    var root = ""
    var bootFiles: [String:String] = [:]
    var kernel = ""
    var initrd = ""
    var dtb = ""
    var arguments = ""
    var media: String?
    var status: String?
    var notes: String?
    var bootEvidence: BootEvidence?
    var secureBoot: SecureBootSupport?
    var displayName: String { systemName?.isEmpty == false ? systemName! : name }
    var logoId: String {
        switch systemId ?? "" {
        case "windows11", "windows": "windows11-original"
        case "windows10", "windows8", "windows7", "windowsserver": "windows8-original"
        case "windowsxp", "windowsserver2003": systemId!
        case "kylin", "uos": "linux"
        case "" where kind == "linux": "linux"
        default: ["ubuntu","debian","fedora","archlinux","linuxmint","nixos","alpinelinux","opensuse","centos","redhat","rockylinux","almalinux","linux","aosc","freebsd","netbsd","openbsd","solaris"].contains(systemId ?? "") ? systemId! : ""
        }
    }
}
struct SecureBootChange:Encodable {var enabled:Bool;var mode:String}
struct SecureBootSupport:Codable,Hashable {
    var mode:String?
    var modes:[String]?
    var eligible:Bool
    var enabled:Bool
    var state:String
    var detail:String
    var components:[SecureBootComponent]
}
struct SecureBootComponent:Codable,Hashable {
    var file:String
    var sha256:String
    var signer:String
    var issuer:String
    var certificateSha256:String
}
struct SystemLogo: View {
    @Environment(\.colorScheme) private var colorScheme
    let id: String
    private var logo: NSImage? {
        let theme = colorScheme == .dark ? "dark" : "light"
        let paths: [String?] = [Bundle.main.resourceURL.map{$0.appendingPathComponent("system-icons/\(theme)/\(id).png").path},
                                FileManager.default.currentDirectoryPath + "/resources/system-icons/\(theme)/\(id).png"]
        return paths.compactMap{$0}.compactMap{NSImage(contentsOfFile:$0)}.first
    }
    var body: some View {
        Group {
            if let logo { Image(nsImage:logo).resizable().scaledToFit().padding(6) }
            else { Image(systemName:"opticaldisc").resizable().scaledToFit().padding(9).foregroundStyle(.secondary) }
        }
        .frame(width:42,height:42)
    }
}
struct BootEvidence: Codable, Hashable {
    var config = ""
    var entry = ""
    var kernel = ""
    var initrd = ""
    var source = ""
    var verification = ""
    var diagnostics: [BootDiagnostic]?
    var sourceLabel: String {
        switch source {
        case "http-tree": L10n.t("HTTP 安装树","HTTP install tree")
        case "http-iso": L10n.t("HTTP 原盘","HTTP ISO")
        case "nfs-tree": L10n.t("只读 NFS 文件树","Read-only NFS file tree")
        case "nbd": L10n.t("只读 NBD 原盘","read-only NBD ISO")
        case "local-media": L10n.t("需要本地介质","local media required")
        case "external": L10n.t("外部安装源","external source")
        default: L10n.tr("未识别安装源")
        }
    }
}
struct BootDiagnostic: Codable, Hashable {
    var code = ""
    var detail = ""
    var label: String {
        switch code {
        case "BOOT_CONFIG_FAMILY_LAYOUT": L10n.t("使用家族布局，未读取原默认项","Family layout; original default not read")
        case "SOURCE_STAGE2_MISSING": L10n.t("缺少第二阶段文件","Stage2 file missing")
        case "SOURCE_REPOSITORY_MISSING": L10n.t("未找到软件仓库索引","Repository index missing")
        case "UOS_MANUAL_ONLY": L10n.tr("需要手动配置")
        case "SOURCE_REQUIRED", "SOURCE_UNVERIFIED": L10n.tr("需要配置安装源")
        default: code
        }
    }
}
struct NetworkSettings: Codable {
    var serverIp: String
    var bindIp: String
    var interface: String
    var bootTransport: String
    var serverMac: String
    var ethernetEnabled: Bool
    var dhcpPort: Int
    var tftpPort: Int
    var httpPort: Int
    var poolStart: String
    var poolEnd: String
    var subnet: String
    var router: String?
    var dhcpAuto: Bool
    var dhcpEnabled: Bool
    var proxyDhcpEnabled: Bool
    var pxePort: Int
    var risSmbPort: Int?
    var smbMode: String?
}
struct NetworkAddress: Codable { var ip:String; var subnet:String }
struct NetworkInterfaceInfo: Codable, Identifiable {
    var id:String; var name:String; var mac:String; var up:Bool; var physical:Bool
    var addresses:[NetworkAddress]; var dhcpServers:[String]; var source:String
}
struct NetworkReport: Codable {
    var interfaces:[NetworkInterfaceInfo]; var selected:String; var address:NetworkAddress?
    var status:String; var source:String; var servers:[String]; var detail:String; var checkedAt:UInt64; var showInterfaceSelector:Bool?
    var summary:String { ((status == "lease_found" ? [] : [L10n.tr(status)])+[selected]+servers).filter{!$0.isEmpty}.joined(separator:"  ") }
}
struct NetworkChoice: Identifiable {
    var interface:NetworkInterfaceInfo; var address:NetworkAddress?
    var id:String { interface.id+"|"+(address?.ip ?? "") }
    var label:String { "\(interface.name)  \(address?.ip ?? L10n.tr("无 IPv4"))  \(L10n.tr(interface.up ? "已连接" : "未连接网卡"))"+(interface.physical ? "" : "  "+L10n.tr("虚拟网卡")) }
}
struct Activity: Codable, Identifiable {
    var time: UInt64
    var device: String
    var `protocol`: String
    var message: String
    var id: String { "\(time)-\(device)-\(message)" }
}
struct Snapshot: Codable {
    var settings: NetworkSettings
    var devices: [Machine]
    var profiles: [SystemImage]
    var defaultProfiles: [String:String]
    var events: [Activity]
    var running: Bool
    var network: NetworkReport?
    var automations: [String:AutomationSelection]?
    var deviceConflicts: [DeviceConflict]?
    var coreVersion: CoreVersion?
}
struct CoreVersion: Codable {var display:String;var build:Int}
struct DeviceConflict: Codable, Identifiable {
    var id:String;var oldId:String;var oldMac:String;var newMac:String;var ip:String?;var oldArchitecture:String;var newArchitecture:String;var reason:String
}
struct AutomationSelection: Codable {var method:String;var filename:String;var profileId:String}
struct ManualBoot: Codable {
    var firmware = ""
    var title = ""
    var status = ""
    var commands = ""
    var notes: [String] = []
}
enum L10n {
    static var zh: Bool { Locale.preferredLanguages.first?.lowercased().hasPrefix("zh") ?? false }
    static func t(_ chinese: String, _ english: String) -> String { let value=Strings.tr(chinese); return !zh && value == chinese ? english : value }
    static func tr(_ chinese: String) -> String { Strings.tr(chinese) }
    static func message(_ text:String)->String {Strings.message(text)}
}
enum AccessGuidance {
    static func downloadURL(_ message:String) -> URL? {
        for url in ["https://npcap.com/#download","https://www.wireshark.org/download.html"] where message.contains(url) {return URL(string:url)}
        return nil
    }
    static func describe(_ message:String) -> String {
        var result=message
        for key in [
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
        ] {result=result.replacingOccurrences(of:key,with:L10n.tr(key))}
        return L10n.message(result.components(separatedBy:"\n").map{L10n.tr($0)}.joined(separator:"\n"))
    }
}
struct NetworkPermissions:Codable {var helperAvailable:Bool;var reason:String;var captureReady:Bool? = nil;var platform:String = "macos";var issues:[String] = []}
@MainActor @Observable final class Server {
    private var apiToken:String = ""
    private let session = URLSession(configuration:.default,delegate:NoRedirect(),delegateQueue:nil)
    private func fetch(url:URL) async throws -> (Data,URLResponse) {try await fetch(request:URLRequest(url:url))}
    private func fetch(request:URLRequest) async throws -> (Data,URLResponse) {var request=request;request.setValue("Bearer \(apiToken)",forHTTPHeaderField:"Authorization");let (data,response)=try await session.data(for:request);if let http=response as? HTTPURLResponse,(200..<300).contains(http.statusCode),http.value(forHTTPHeaderField:"X-Bootah-Authentication") != "bearer-v1" {throw NSError(domain:"Bootah",code:401,userInfo:[NSLocalizedDescriptionKey:L10n.tr("核心未确认令牌鉴权")])};return (data,response)}
    private func externalToken() -> String {
        let args=ProcessInfo.processInfo.arguments
        if let index=args.firstIndex(of:"--api-token"),index+1<args.count{return args[index+1]}
        if let token=ProcessInfo.processInfo.environment["BOOTAH_API_TOKEN"] {return token}
        let query:[String:Any]=[kSecClass as String:kSecClassGenericPassword,kSecAttrService as String:"work.premutation.bootah.api",kSecAttrAccount as String:endpoint.absoluteString,kSecReturnData as String:true]
        var result:CFTypeRef?
        if SecItemCopyMatching(query as CFDictionary,&result)==errSecSuccess,let data=result as? Data,let token=String(data:data,encoding:.utf8){return token}
        return askToken()
    }
    func askToken() -> String {
        let query:[String:Any]=[kSecClass as String:kSecClassGenericPassword,kSecAttrService as String:"work.premutation.bootah.api",kSecAttrAccount as String:endpoint.absoluteString]
        let alert=NSAlert();alert.messageText=L10n.tr("管理令牌");alert.addButton(withTitle:L10n.tr("确定"));alert.addButton(withTitle:L10n.tr("取消"));let field=NSSecureTextField(frame:NSRect(x:0,y:0,width:320,height:24));alert.accessoryView=field
        guard alert.runModal() == .alertFirstButtonReturn else{return ""}
        let token=field.stringValue
        if !token.isEmpty {let status=SecItemUpdate(query as CFDictionary,[kSecValueData as String:Data(token.utf8)] as CFDictionary);if status==errSecItemNotFound {var add=query;add[kSecValueData as String]=Data(token.utf8);SecItemAdd(add as CFDictionary,nil)}}
        return token
    }
    func editToken(){let token=askToken();if !token.isEmpty {apiToken=token;Task{await refresh()}}}
    var networkPermissions:NetworkPermissions?
    var startupPermissionsShown=false
    var checkingPermissions=false
    var starting=false
    var startAfterNetworkApproval=false
    var showNetworkPermissions=false
    var bundledCore:Bool {ProcessInfo.processInfo.environment["NETBOOT_API"] == nil && ProcessInfo.processInfo.environment["NETBOOT_CORE"] == nil && Bundle.main.bundleURL.pathExtension == "app"}
    var helperAvailable:Bool {bundledCore && networkPermissions?.helperAvailable == true}
    var snapshot: Snapshot?
    var error: String?
    var selected: String = "machines"
    var uboot = ""
    var manual: ManualBoot?
    var connected = false
    var automationCapabilities: [String:[String]] = [:]
    private var capabilityProfiles: [String:SystemImage] = [:]
    var detected = false
    var detecting = false
    var process: Process?
    private var coreLog: FileHandle?
    private let endpoint = URL(string: ProcessInfo.processInfo.environment["NETBOOT_API"] ?? "http://127.0.0.1:9080")!
    var coreBinary: String { ProcessInfo.processInfo.environment["NETBOOT_CORE"] ?? Bundle.main.bundleURL.appendingPathComponent("Contents/MacOS/netboot-core").path }
    var coreConfig: String { ProcessInfo.processInfo.environment["NETBOOT_CONFIG"] ?? FileManager.default.urls(for:.applicationSupportDirectory,in:.userDomainMask)[0].appendingPathComponent("Netboot/state.json").path }
    func launch() {
        guard process?.isRunning != true else {return}
        let bundled=Bundle.main.bundleURL.appendingPathComponent("Contents/MacOS/netboot-core").path
        let executable=ProcessInfo.processInfo.environment["NETBOOT_CORE"] ?? bundled
        guard ProcessInfo.processInfo.environment["NETBOOT_API"] == nil else {apiToken=externalToken();return}
        let args=ProcessInfo.processInfo.arguments
        if let index=args.firstIndex(of:"--api-token"),index+1<args.count {apiToken=args[index+1]}
        else if let token=ProcessInfo.processInfo.environment["BOOTAH_API_TOKEN"] {apiToken=token}
        else {var bytes=[UInt8](repeating:0,count:32);guard SecRandomCopyBytes(kSecRandomDefault,bytes.count,&bytes)==errSecSuccess else {self.error=L10n.tr("无法生成管理令牌");return};apiToken=bytes.map{String(format:"%02x",$0)}.joined()}
        guard FileManager.default.isExecutableFile(atPath:executable) else {self.error="The Bootah core is missing or cannot be executed. Download and reinstall the complete Bootah package for this platform, or correct NETBOOT_CORE and its execute permissions.";return}
        let directory=FileManager.default.urls(for:.applicationSupportDirectory,in:.userDomainMask)[0].appendingPathComponent("Netboot")
        do {try FileManager.default.createDirectory(at:directory,withIntermediateDirectories:true)} catch {self.error=error.localizedDescription;return}
        let process = Process()
        process.executableURL = URL(fileURLWithPath: executable)
        process.arguments = ["--config", coreConfig, "--exit-on-stdin-close", "--api-token-stdin"]
        process.standardInput=Pipe()
        let pipe=Pipe();process.standardOutput=pipe;process.standardError=pipe
        let log=URL(fileURLWithPath:coreConfig).deletingLastPathComponent().appendingPathComponent("core.log")
        if !FileManager.default.fileExists(atPath:log.path) { FileManager.default.createFile(atPath:log.path,contents:nil) }
        coreLog=try? FileHandle(forWritingTo:log)
        if let coreLog, (try? coreLog.seekToEnd()) ?? 0 > 4*1024*1024 { try? coreLog.truncate(atOffset:0) }
        let logger=coreLog
        pipe.fileHandleForReading.readabilityHandler={ handle in let data=handle.availableData;if !data.isEmpty,let logger {if (try? logger.seekToEnd()) ?? 0 > 4*1024*1024 {try? logger.truncate(atOffset:0)};try? logger.write(contentsOf:data)} }
        do { try process.run();(process.standardInput as? Pipe)?.fileHandleForWriting.write(Data((apiToken+"\n").utf8));self.process = process } catch { self.error = "The Bootah core is missing or cannot be executed. Download and reinstall the complete Bootah package for this platform, or correct NETBOOT_CORE and its execute permissions."+"\n"+error.localizedDescription }
    }
    func diagnostics(level:String,includePrivate:Bool) throws -> Data {
        let process=Process();process.executableURL=URL(fileURLWithPath:coreBinary)
        process.arguments=["--export-diagnostics","--config",coreConfig,"--log-level",level]+(includePrivate ? ["--include-private"] : [])
        let output=Pipe();process.standardOutput=output;let errors=Pipe();process.standardError=errors
        try process.run();let data=output.fileHandleForReading.readDataToEndOfFile();process.waitUntilExit()
        if process.terminationStatus != 0 {throw NSError(domain:"Bootah",code:Int(process.terminationStatus),userInfo:[NSLocalizedDescriptionKey:String(data:errors.fileHandleForReading.readDataToEndOfFile(),encoding:.utf8) ?? "Export failed"])}
        return data
    }
    var networkHelper:SMAppService {SMAppService.daemon(plistName:"work.premutation.bootah.network.plist")}
    func authorizeNetworkHelper() async {
        await refreshNetworkPermissions()
        guard helperAvailable else{return}
        do {
            if networkHelper.status == .notRegistered || networkHelper.status == .notFound {try networkHelper.register()}
            if networkHelper.status == .requiresApproval {SMAppService.openSystemSettingsLoginItems()}
            else if networkHelper.status == .enabled {showNetworkPermissions=false;await detectNetwork(authorize:true)}
        } catch {self.error=L10n.tr("无法启用网络助手。请检查系统设置中的后台项目许可。")+"\n"+error.localizedDescription}
    }
    func confirmNetworkAccess() async {
        guard !starting && snapshot?.running != true else {showNetworkPermissions=false;return}
        showNetworkPermissions=false
        if helperAvailable {
            await authorizeNetworkHelper()
            if networkHelper.status == .requiresApproval {
                startAfterNetworkApproval=true
                return
            }
            guard networkHelper.status == .enabled else {return}
        }
        _ = await send("api/service/start",[String:String]())
    }
    func disableNetworkHelper() async {
        guard helperAvailable && snapshot?.running != true else{return}
        do {try await networkHelper.unregister();await refreshNetworkPermissions()}catch{self.error=error.localizedDescription}
    }
    func refreshNetworkPermissions() async {
        guard !checkingPermissions else{return};checkingPermissions=true
        defer{checkingPermissions=false}
        do {
            var request=URLRequest(url:endpoint.appendingPathComponent("api/network/permissions"));request.timeoutInterval=40
            let (data,response)=try await fetch(request:request)
            guard (response as? HTTPURLResponse)?.statusCode==200 else{throw URLError(.badServerResponse)}
            let decoder=JSONDecoder();decoder.keyDecodingStrategy = .convertFromSnakeCase
            networkPermissions=try decoder.decode(NetworkPermissions.self,from:data)
        } catch {networkPermissions=NetworkPermissions(helperAvailable:false,reason:"unavailable",issues:["Network permission checks could not complete. Check the core connection and retry from network settings."])}
    }
    func refresh() async {
        do {
            let (bytes, response) = try await fetch(url: endpoint.appendingPathComponent("api/state"))
            guard (response as? HTTPURLResponse)?.statusCode == 200 else { connected=false; return }
            let decoder = JSONDecoder(); decoder.keyDecodingStrategy = .convertFromSnakeCase
            snapshot = try decoder.decode(Snapshot.self, from: bytes)
            connected=true
            for profile in snapshot?.profiles ?? [] where capabilityProfiles[profile.id] != profile {
                automationCapabilities[profile.id]=nil
                do {
                    let (data,response)=try await fetch(url:endpoint.appendingPathComponent("api/profiles/\(profile.id)/automation-methods"))
                    guard (response as? HTTPURLResponse)?.statusCode==200 else {continue}
                    automationCapabilities[profile.id]=try JSONDecoder().decode([String].self,from:data)
                    capabilityProfiles[profile.id]=profile
                } catch {continue}
            }
            if startAfterNetworkApproval && networkHelper.status == .enabled {
                startAfterNetworkApproval=false
                if snapshot?.running != true {_ = await send("api/service/start",[String:String]())}
            }
            if !detected && snapshot?.running == false { detected=true; await detectNetwork() }
            if !startupPermissionsShown && !detecting {
                startupPermissionsShown=true;await refreshNetworkPermissions()
                if !(networkPermissions?.issues.isEmpty ?? true) {showNetworkPermissions=true}
            }
        } catch { connected=false }
    }
    func detectNetwork(authorize:Bool=false) async {
        guard !detecting else{return}; detecting=true
        _ = await send("api/network/detect",["authorize":authorize]); detecting=false
    }
    func send<T:Encodable>(_ path: String, _ value: T, method: String = "POST") async -> Bool {
        let startsService=path=="api/service/start"
        if startsService && starting {return false}
        if startsService {starting=true}
        defer {if startsService {starting=false}}
        do {
            if startsService && bundledCore {
                await refreshNetworkPermissions()
                if helperAvailable && !(networkPermissions?.issues.isEmpty ?? true) {
                    if networkHelper.status == .notRegistered || networkHelper.status == .notFound {try networkHelper.register()}
                    if networkHelper.status != .enabled {
                        if networkHelper.status == .requiresApproval {SMAppService.openSystemSettingsLoginItems()}
                        self.error=L10n.tr("请在系统设置中允许网络助手后，再点击启动。")
                        showNetworkPermissions=true
                        return false
                    }
                }
            }
            var request = URLRequest(url: endpoint.appendingPathComponent(path))
            request.httpMethod = method
            if path=="api/service/start" || path=="api/network/detect" {request.timeoutInterval=150}
            if path.hasSuffix("/compatibility") || path.contains("/secure-boot") {request.timeoutInterval=1800}
            request.setValue("application/json", forHTTPHeaderField: "Content-Type")
            let encoder = JSONEncoder(); encoder.keyEncodingStrategy = .convertToSnakeCase
            request.httpBody = try encoder.encode(value)
            let (bytes, response) = try await fetch(request: request)
            guard let http = response as? HTTPURLResponse, (200..<300).contains(http.statusCode) else {
                self.error = String(decoding: bytes, as: UTF8.self); return false
            }
            await refresh(); return true
        } catch { self.error = error.localizedDescription; return false }
    }
    var changingSmb = false
    func configureSmb() async {
        guard !changingSmb else {return}
        changingSmb=true
        defer {changingSmb=false}
        do {
            let url=endpoint.appendingPathComponent("api/smb")
            let (bytes,response)=try await fetch(url:url)
            guard let http=response as? HTTPURLResponse,(200..<300).contains(http.statusCode) else {self.error=String(decoding:bytes,as:UTF8.self);return}
            let state=try JSONSerialization.jsonObject(with:bytes) as? [String:Any] ?? [:]
            let modes=["off","auto","smb1","smb2"]
            let labels=[L10n.t("关闭","Off"),L10n.t("自动（按镜像）","Automatic (by image)"),"SMB1 — XP / Server 2003","SMB2 — Vista / Server 2008+"]
            let current=state["mode"] as? String ?? "off"
            let alert=NSAlert();alert.messageText=L10n.t("Windows 安装源服务","Windows installation source")
            alert.informativeText=L10n.t("当前监听：","Listening: ")+current+"  "+(state["bind_ip"] as? String ?? "")+":"+String(describing:state["port"] ?? "")
            let popup=NSPopUpButton(frame:NSRect(x:0,y:0,width:340,height:28));popup.addItems(withTitles:labels);popup.selectItem(at:modes.firstIndex(of:state["configured_mode"] as? String ?? current) ?? 0)
            alert.accessoryView=popup;alert.addButton(withTitle:L10n.t("应用","Apply"));alert.addButton(withTitle:L10n.t("取消","Cancel"))
            guard alert.runModal() == .alertFirstButtonReturn else{return}
            let mode=modes[popup.indexOfSelectedItem]
            var payload:[String:Any]=["mode":mode,"confirm":false]
            while true {
                var request=URLRequest(url:url);request.httpMethod="POST";request.setValue("application/json",forHTTPHeaderField:"Content-Type");request.httpBody=try JSONSerialization.data(withJSONObject:payload)
                let (data,result)=try await fetch(request:request)
                guard let http=result as? HTTPURLResponse else{return}
                if (200..<300).contains(http.statusCode) {await refresh();return}
                let conflict=(try? JSONSerialization.jsonObject(with:data)) as? [String:Any]
                guard http.statusCode==409,conflict?["code"] as? String == "smb_switch_confirmation",let old=conflict?["current_mode"] as? String else {self.error=String(decoding:data,as:UTF8.self);return}
                let confirm=NSAlert();confirm.alertStyle = .warning;confirm.messageText=L10n.t("停止当前安装源服务？","Stop the current installation source?")
                confirm.informativeText=(conflict?[L10n.zh ? "message_zh":"message"] as? String ?? "")+"\n"+L10n.t("正在进行的 Windows 安装可能中断。","Windows installations in progress may be interrupted.")
                confirm.addButton(withTitle:mode=="off" ? L10n.t("停止服务","Stop service"):L10n.t("停止并切换","Stop and switch"));confirm.addButton(withTitle:L10n.t("取消","Cancel"))
                guard confirm.runModal() == .alertFirstButtonReturn else{return}
                payload=["mode":mode,"confirm":true,"expected_mode":old]
            }
        } catch {self.error=error.localizedDescription}
    }
    func importISO(_ url: URL, trustLocalISO: Bool = false) async -> (ids:[String],summary:String) {
        do {var request=URLRequest(url:endpoint.appendingPathComponent("api/media/import"));request.httpMethod="POST";request.timeoutInterval=1800;request.setValue("application/json",forHTTPHeaderField:"Content-Type");request.httpBody=try JSONSerialization.data(withJSONObject:["path":url.path,"trust_local_iso":trustLocalISO]);let (bytes,response)=try await fetch(request:request);guard let http=response as? HTTPURLResponse,(200..<300).contains(http.statusCode) else {self.error=String(decoding:bytes,as:UTF8.self);return ([],"")};let result=try JSONSerialization.jsonObject(with:bytes) as? [String:Any];let profiles=result?["profiles"] as? [[String:Any]] ?? [];let warnings=(result?["warnings"] as? [String] ?? []);let summary=warnings.map(L10n.message).joined(separator:"\n\n");await refresh();return (profiles.compactMap{$0["id"] as? String},summary)}catch{self.error=error.localizedDescription;return ([],"")}
    }
    func automationMethods(_ id:String) async -> [String] {do {let (data,_) = try await fetch(url:endpoint.appendingPathComponent("api/profiles/\(id)/automation-methods"));return try JSONDecoder().decode([String].self,from:data)}catch{self.error=error.localizedDescription;return []}}
    func saveAutomation(scope:String,id:String,method:String,file:URL) async {do {var parts=URLComponents(url:endpoint.appendingPathComponent("api/automation/\(scope)/\(id)"),resolvingAgainstBaseURL:false)!;parts.queryItems=[URLQueryItem(name:"method",value:method),URLQueryItem(name:"filename",value:file.lastPathComponent)];var request=URLRequest(url:parts.url!);request.httpMethod="PUT";request.httpBody=try Data(contentsOf:file);let (data,response)=try await fetch(request:request);guard (response as? HTTPURLResponse)?.statusCode == 204 else{self.error=String(decoding:data,as:UTF8.self);return};await refresh()}catch{self.error=error.localizedDescription}}
    func clearAutomation(scope:String,id:String) async {var request=URLRequest(url:endpoint.appendingPathComponent("api/automation/\(scope)/\(id)"));request.httpMethod="DELETE";do{let (_,response)=try await fetch(request:request);if (response as? HTTPURLResponse)?.statusCode == 204 {await refresh()}}catch{self.error=error.localizedDescription}}
    func commands(_ id: String) async {
        do { let (bytes, _) = try await fetch(url:endpoint.appendingPathComponent("api/profiles/\(id)/uboot")); uboot=String(decoding:bytes,as:UTF8.self) } catch { self.error=error.localizedDescription }
    }
    func parameters(_ image:SystemImage,preview:Bool=false) async -> ParameterInfo? {
        do {
            var request=URLRequest(url:endpoint.appendingPathComponent("api/profiles/\(image.id)/parameters"))
            if preview {let encoder=JSONEncoder();encoder.keyEncodingStrategy = .convertToSnakeCase;request.httpMethod="POST";request.httpBody=try encoder.encode(image);request.setValue("application/json",forHTTPHeaderField:"Content-Type")}
            let (data,response)=try await fetch(request:request)
            guard let http=response as? HTTPURLResponse,(200..<300).contains(http.statusCode) else {self.error=L10n.message(String(decoding:data,as:UTF8.self));return nil}
            let decoder=JSONDecoder();decoder.keyDecodingStrategy = .convertFromSnakeCase;return try decoder.decode(ParameterInfo.self,from:data)
        }catch {self.error=error.localizedDescription;return nil}
    }
    func secureBootSupport(_ id:String) async -> SecureBootSupport? {
        do {
            let (data,response)=try await fetch(url:endpoint.appendingPathComponent("api/profiles/\(id)/secure-boot"))
            guard let http=response as? HTTPURLResponse,(200..<300).contains(http.statusCode) else {self.error=String(decoding:data,as:UTF8.self);return nil}
            let decoder=JSONDecoder();decoder.keyDecodingStrategy = .convertFromSnakeCase
            return try decoder.decode(SecureBootSupport.self,from:data)
        } catch {self.error=error.localizedDescription;return nil}
    }
    func manualCommands(_ id: String) async {
        do {
            var components=URLComponents(url:endpoint.appendingPathComponent("api/profiles/\(id)/boot-commands"),resolvingAgainstBaseURL:false)
            components?.queryItems=[URLQueryItem(name:"lang",value:L10n.zh ? "zh" : "en")]
            guard let url=components?.url else {return}
            let (bytes, _) = try await fetch(url:url)
            manual=try JSONDecoder().decode(ManualBoot.self,from:bytes)
        } catch { self.error=error.localizedDescription }
    }
}
@main struct NetbootApp: App {
    @State private var server = Server()
    var body: some Scene {
        WindowGroup {
            MainView(server:server).frame(minWidth:780,minHeight:500)
                .task { server.launch(); while !Task.isCancelled { await server.refresh(); try? await Task.sleep(for:.seconds(3)) } }
                .onReceive(NotificationCenter.default.publisher(for:NSApplication.willTerminateNotification)) { _ in
                    if server.process?.isRunning == true {server.process?.terminate()}
                }
                .alert("Bootah",isPresented:Binding(get:{server.error != nil},set:{if !$0 {server.error=nil}})) { if (server.error ?? "").contains("ChmodBPF") {Button(L10n.tr("网络权限")){server.showNetworkPermissions=true}}; if let url=AccessGuidance.downloadURL(server.error ?? "") {Button(L10n.t("打开官方下载页面","Open official download page")){NSWorkspace.shared.open(url)}}; Button(L10n.tr("好")) {server.error=nil} } message: {Text(AccessGuidance.describe(L10n.tr(server.error ?? "")))}
        }.defaultSize(width:1000,height:660)
    }
}
struct ArchitectureDefaultRow:Identifiable {let id:String}
struct MainView: View {
    @Bindable var server: Server
    @State var selectedMachine:String?
    @State var machine: Machine?
    @State private var deletingSelection=false
    @State var selectedImage:String?
    @State var image: SystemImage?
    @State private var importOptions = false
    @State private var importURL: URL?
    @State private var importing = false
    @State var network = false
    @State var about = false
    @State var commands = false
    @State var manualCommands = false
    @State private var showingDeviceConflict = false
    private func confirmDeviceConflict(_ conflict:DeviceConflict) {
        guard !showingDeviceConflict else{return};showingDeviceConflict=true
        let details=conflict.reason=="ip-mac" ? "IP \(conflict.ip ?? ""): \(conflict.oldMac) → \(conflict.newMac)" : "\(conflict.newMac): \(conflict.oldArchitecture) → \(conflict.newArchitecture)"
        let alert=NSAlert();alert.messageText=L10n.tr("清理旧设备记录？");alert.informativeText=details+"\n"+L10n.tr("旧设备有手动配置。清理后，新设备将在下次网络启动时使用默认镜像。")
        alert.addButton(withTitle:L10n.tr("清理"));alert.addButton(withTitle:L10n.tr("保留"))
        let accept=alert.runModal() == .alertFirstButtonReturn;showingDeviceConflict=false
        Task {_ = await server.send("api/device-conflicts/\(conflict.id)",["accept":accept])}
    }
    @State private var logLevel = "info"
    @State private var includePrivate = false
    private var logOptions: some View {
        HStack(spacing:20) {
            Picker(L10n.t("日志级别","Log level"),selection:$logLevel) {
                Text(L10n.t("错误","Errors")).tag("error")
                Text(L10n.t("常规","Standard")).tag("info")
                Text(L10n.t("详细","Detailed")).tag("debug")
            }.frame(width:210)
            Toggle(L10n.t("包含原始隐私信息","Include private data"),isOn:$includePrivate).toggleStyle(.checkbox).fixedSize()
        }
    }
    private var logExport: some View {
        Button(L10n.t("导出诊断日志","Export diagnostics"),action:exportDiagnostics).fixedSize()
    }
    private func exportDiagnostics() {
        if includePrivate {
            let alert=NSAlert();alert.messageText=L10n.t("原始日志包含隐私信息","Raw logs contain private data")
            alert.informativeText=L10n.t("原始日志可能包含设备地址和本地文件路径。","Raw logs may contain device addresses and local file paths.")
            alert.addButton(withTitle:L10n.t("继续","Continue"));alert.addButton(withTitle:L10n.t("取消","Cancel"))
            guard alert.runModal() == .alertFirstButtonReturn else{return}
        }
        let panel=NSSavePanel();panel.nameFieldStringValue="bootah-diagnostics.txt";panel.allowedContentTypes=[.plainText]
        guard panel.runModal() == .OK,let url=panel.url else{return}
        do {try server.diagnostics(level:logLevel,includePrivate:includePrivate).write(to:url,options:.atomic)} catch {server.error=error.localizedDescription}
    }
    private func selectISO() {
        let panel=NSOpenPanel();panel.canChooseDirectories=false;panel.allowsMultipleSelection=false
        panel.allowedContentTypes=[UTType(filenameExtension:"iso") ?? .data]
        guard panel.runModal() == .OK,let url=panel.url else{return}
        importURL=url;importOptions=true
    }
    private func startImport(url:URL,trusted:Bool){
        importing=true
        Task {let result=await server.importISO(url,trustLocalISO:trusted);importing=false
            if !result.summary.isEmpty {let notice=NSAlert();notice.messageText=L10n.tr("需要处理的问题");notice.informativeText=result.summary;notice.addButton(withTitle:L10n.tr("确定"));notice.runModal()}
        }
    }
    private func automationButton(scope:String,id:String,label:String)->some View {
        let profileId=scope=="profile" ? id : server.snapshot?.devices.first(where:{$0.id==id}).flatMap{$0.profileId ?? server.snapshot?.defaultProfiles[$0.architecture]}
        let methods=profileId.flatMap{server.automationCapabilities[$0]}
        let enabled=methods?.isEmpty == false
        return HStack {
            Button(label){Task{await configureAutomation(scope:scope,id:id)}}.disabled(!enabled)
        }.contentShape(Rectangle()).help(enabled ? "" : L10n.tr(profileId==nil ? "请先分配启动镜像" : methods==nil ? "正在读取自动配置选项…" : "该平台不支持自动配置"))
    }
    private func configureAutomation(scope:String,id:String) async {
        let profileId:String
        if scope == "profile" {profileId=id}
        else {guard let device=server.snapshot?.devices.first(where:{$0.id==id}),let selected=device.profileId ?? server.snapshot?.defaultProfiles[device.architecture] else{return};profileId=selected}
        let methods=await server.automationMethods(profileId)
        guard !methods.isEmpty else{return}
        let current=server.snapshot?.automations?["\(scope):\(id)"]
        let alert=NSAlert();alert.messageText=L10n.t("自动安装配置","Automatic installation");alert.informativeText=current.map{$0.filename} ?? L10n.t("选择配置机制及配置文件","Choose a method and configuration file")
        let popup=NSPopUpButton(frame:NSRect(x:0,y:0,width:320,height:28));popup.addItems(withTitles:[L10n.t("不使用自动配置","No automatic configuration")]+methods);if let current,let index=methods.firstIndex(of:current.method){popup.selectItem(at:index+1)};alert.accessoryView=popup;alert.addButton(withTitle:L10n.t("确定","OK"));alert.addButton(withTitle:L10n.t("取消","Cancel"))
        guard alert.runModal() == .alertFirstButtonReturn else{return}
        guard popup.indexOfSelectedItem > 0 else {await server.clearAutomation(scope:scope,id:id);return}
        let panel=NSOpenPanel();panel.canChooseDirectories=false;panel.allowsMultipleSelection=false
        guard panel.runModal() == .OK,let file=panel.url else{return}
        await server.saveAutomation(scope:scope,id:id,method:methods[popup.indexOfSelectedItem-1],file:file)
    }
    var body: some View {
        NavigationSplitView {
            List(selection:$server.selected) {
                Label(L10n.t("设备与 Profile","Devices & profiles"),systemImage:"desktopcomputer").tag("machines")
                Label(L10n.t("介质与导入","Media & import"),systemImage:"externaldrive").tag("systems")
                Label(L10n.t("传输与日志","Transfers & logs"),systemImage:"list.bullet.rectangle").tag("activity")
            }.navigationSplitViewColumnWidth(min:180,ideal:210)
            .safeAreaInset(edge:.bottom) {
                VStack(alignment:.leading,spacing:8) {
                    Text(server.detecting ? L10n.tr("检测中…") : server.snapshot?.network?.summary ?? "").font(.caption).foregroundStyle(.secondary).help(AccessGuidance.describe(server.snapshot?.network?.detail ?? ""))
                    Button(L10n.tr("重新检测")){Task{await server.detectNetwork()}}
                    if server.helperAvailable {Button(L10n.tr("授权检测")){Task{await server.authorizeNetworkHelper()}}}
                }.padding().disabled(server.detecting || server.snapshot?.running == true)
            }
        } detail: {
            Group {
                switch server.selected {
                case "machines":
                    Table(server.snapshot?.devices ?? [],selection:$selectedMachine) {
                        TableColumn(L10n.t("设备","Device")) {d in Button(d.name.isEmpty ? (d.mac.isEmpty ? (d.ip ?? L10n.tr("IP 绑定")) : d.mac) : d.name) {machine=d}.buttonStyle(.plain)}
                        TableColumn(L10n.t("地址","Address")) {Text($0.ip ?? $0.mac)}
                        TableColumn(L10n.t("架构","Architecture")) {Text($0.architecture)}
                        TableColumn(L10n.t("观察到的型号","Observed model")) {Text($0.model.isEmpty ? "—" : $0.model)}
                        TableColumn(L10n.t("启动镜像","Boot image")) {d in
                            Picker(L10n.t("启动镜像","Boot image"),selection:Binding(
                                get:{d.profileId ?? ""},
                                set:{id in var changed=d;changed.profileId=id.isEmpty ? nil : id;Task {_ = await server.send("api/devices",changed)}})) {
                                Text((server.snapshot?.defaultProfiles[d.architecture]).flatMap{id in server.snapshot?.profiles.first(where:{$0.id==id})?.name}.map{L10n.tr("按架构自动分配")+"  "+$0} ?? L10n.tr("未分配")).tag("")
                                ForEach(server.snapshot?.profiles ?? []) {profile in Text(profile.name).tag(profile.id)}
                            }.labelsHidden().frame(maxWidth:.infinity,alignment:.leading)
                        }
                        TableColumn(L10n.t("自动安装配置","Automatic installation")) {d in automationButton(scope:"device",id:d.id,label:server.snapshot?.automations?["device:\(d.id)"]?.filename ?? L10n.t("选择配置","Choose config"))}
                    }
                case "systems":
                    VStack {
                        Text(L10n.tr("架构默认启动镜像")).font(.headline).frame(maxWidth:.infinity,alignment:.leading).padding(.horizontal)
                        Table(Array(Set((server.snapshot?.profiles ?? []).flatMap{$0.bootFiles.keys}).intersection(FirmwareKeys.all)).sorted().map{ArchitectureDefaultRow(id:$0)}) {
                            TableColumn(L10n.tr("架构")) {row in Text(row.id)}.width(min:120,ideal:170,max:220)
                            TableColumn(L10n.tr("启动镜像")) {row in
                                Picker(L10n.tr("启动镜像"),selection:Binding(
                                    get:{server.snapshot?.defaultProfiles[row.id] ?? ""},
                                    set:{id in Task {var defaults=server.snapshot?.defaultProfiles ?? [:];if id.isEmpty {defaults.removeValue(forKey:row.id)} else {defaults[row.id]=id};_ = await server.send("api/default-profiles",defaults,method:"PUT")}})) {
                                    Text(L10n.tr("未设置")).tag("")
                                    ForEach((server.snapshot?.profiles ?? []).filter{$0.bootFiles[row.id] != nil}) {p in Text(p.name).lineLimit(1).truncationMode(.middle).tag(p.id)}
                                }.labelsHidden().lineLimit(1).truncationMode(.middle).frame(width:320,alignment:.leading)
                                    .help(server.snapshot?.profiles.first(where:{$0.id==server.snapshot?.defaultProfiles[row.id]})?.name ?? L10n.tr("未设置"))
                            }
                        }.frame(height: min(240, CGFloat(Set((server.snapshot?.profiles ?? []).flatMap{$0.bootFiles.keys}).intersection(FirmwareKeys.all).count * 32 + 32)))
                    List(server.snapshot?.profiles ?? [],selection:$selectedImage) {p in
                        HStack { SystemLogo(id:p.logoId); VStack(alignment:.leading) {Button(p.displayName) {image=p}.buttonStyle(.plain); Text(p.name).font(.caption).foregroundStyle(.secondary)}; Spacer(); Text(p.architecture).foregroundStyle(.secondary); automationButton(scope:"profile",id:p.id,label:server.snapshot?.automations?["profile:\(p.id)"]?.filename ?? L10n.tr("自动安装配置")); Button(L10n.tr("镜像设置")){image=p}; Button(L10n.tr("启动方式")) {Task {await server.manualCommands(p.id);manualCommands=true}} }.tag(p.id).accessibilityElement(children:.contain)
                    }
                    }
                default:
                    ScrollViewReader { logScroll in
                    VStack(alignment:.leading) {
                        HStack {
                            Button(L10n.tr("清除日志")) {Task {_ = await server.send("api/events",[String:String](),method:"DELETE")}}
                            Button(L10n.tr("回到日志顶部")) {if let index=server.snapshot?.events.indices.last {logScroll.scrollTo(index,anchor:.top)}}
                        }.padding(.horizontal,16)
                        ViewThatFits(in:.horizontal) {
                            HStack(spacing:20) { logOptions; Spacer(minLength:24); logExport }
                            VStack(alignment:.leading,spacing:12) { logOptions; logExport }
                        }.padding(.horizontal,16).padding(.vertical,8)
                        List(Array((server.snapshot?.events ?? []).enumerated().reversed()), id:\.offset) {_,event in
                            HStack {Text(Date(timeIntervalSince1970:Double(event.time)),style:.time).monospacedDigit().foregroundStyle(.secondary);Text(event.device);Text(event.protocol).foregroundStyle(.secondary);Text(event.message)}
                        }
                    }
                    }
                }
            }.navigationTitle(server.selected=="machines" ? L10n.t("设备与 Profile","Devices & profiles") : server.selected=="systems" ? L10n.t("介质与导入","Media & import") : L10n.t("传输与日志","Transfers & logs"))
                .toolbar {
                    if server.selected=="systems" {
                        Button(L10n.tr(importing ? "正在导入…" : "导入 ISO"),systemImage:"square.and.arrow.down"){selectISO()}.disabled(importing)
                        Menu {Button(L10n.tr("高级手动配置")){image=SystemImage()}} label:{Image(systemName:"ellipsis.circle")}
                    } else if server.selected=="machines" {
                        Button {machine=Machine()} label:{Image(systemName:"plus")}
                    }
                    Button {
                        if server.selected=="machines" {machine=server.snapshot?.devices.first(where:{$0.id==selectedMachine})}
                        else {image=server.snapshot?.profiles.first(where:{$0.id==selectedImage})}
                    } label:{Image(systemName:"pencil")}.disabled(server.selected=="machines" ? selectedMachine==nil : selectedImage==nil)
                    if server.selected=="machines" || server.selected=="systems" {
                        Button(L10n.tr("删除"),systemImage:"trash",role:.destructive) {
                            let devices=server.selected=="machines"
                            guard let id=devices ? selectedMachine : selectedImage else {return}
                            deletingSelection=true
                            Task {
                                if await server.send("api/\(devices ? "devices" : "profiles")/\(id)",[String:String](),method:"DELETE") {
                                    if devices {selectedMachine=nil} else {selectedImage=nil}
                                }
                                deletingSelection=false
                            }
                        }.disabled(deletingSelection || !server.connected || (server.selected=="machines" ? !(server.snapshot?.devices.contains(where:{$0.id==selectedMachine}) ?? false) : !(server.snapshot?.profiles.contains(where:{$0.id==selectedImage}) ?? false)))
                    }
                    Button {Task{await server.configureSmb()}} label:{Image(systemName:"externaldrive.badge.wifi")}.help(L10n.t("Windows 安装源服务","Windows installation source")).disabled(server.changingSmb)
                    Button {network=true} label:{Image(systemName:"network")}
                    Button {server.showNetworkPermissions=true} label:{Image(systemName:"lock.shield")}.help(L10n.tr("网络权限"))
                    if ProcessInfo.processInfo.environment["NETBOOT_API"] != nil {Button(L10n.tr("管理令牌")){server.editToken()}}
                    Button {about=true} label:{Image(systemName:"info.circle")}.help(L10n.t("关于与开源许可","About & open-source licenses"))
                    Text(L10n.tr(server.connected ? (server.snapshot?.running == true ? "运行中" : "已停止") : "未连接")).foregroundStyle(.secondary)
                    Button(L10n.tr(server.snapshot?.running == true ? "停止" : "启动")) {Task {_ = await server.send(server.snapshot?.running == true ? "api/service/stop" : "api/service/start",[String:String]())}}.disabled(server.starting)
                }
        }
        .sheet(item:$machine) {d in MachineEditor(server:server,machine:d)}
        .sheet(isPresented:$importOptions){if let url=importURL {ImportOptionsView(url:url){trusted in startImport(url:url,trusted:trusted)}}}
        .sheet(isPresented:$server.showNetworkPermissions){NetworkPermissionsView(server:server)}
        .sheet(item:$image) {p in ImageEditor(server:server,image:p)}
        .sheet(isPresented:$network) {if let settings=server.snapshot?.settings {NetworkEditor(server:server,settings:settings)}}
        .sheet(isPresented:$about) {AboutView(server:server)}
        .sheet(isPresented:$commands) {VStack {TextEditor(text:$server.uboot).font(.system(.body,design:.monospaced));HStack {Button(L10n.tr("复制")) {NSPasteboard.general.clearContents();NSPasteboard.general.setString(server.uboot,forType:.string)};Spacer();Button(L10n.tr("完成")) {commands=false}}}.padding().frame(width:540,height:300)}
        .sheet(isPresented:$manualCommands) {if let manual=server.manual {BootCommandsView(manual:manual)}}
        .onChange(of:server.snapshot?.deviceConflicts?.first?.id) {_,_ in if let conflict=server.snapshot?.deviceConflicts?.first {confirmDeviceConflict(conflict)} }
    }
}
struct ImportOptionsView:View {
    let url:URL
    var start:(Bool)->Void
    @State private var trusted=false
    @Environment(\.dismiss) private var dismiss
    var body:some View {
        VStack(alignment:.leading,spacing:18){
            Label(L10n.tr("导入 ISO"),systemImage:"opticaldisc").font(.title2)
            Form {
                LabeledContent(L10n.tr("镜像文件")){Text(url.lastPathComponent).textSelection(.enabled).lineLimit(2)}
                Toggle(L10n.tr("信任所选本地 ISO 安装源"),isOn:$trusted).toggleStyle(.switch)
            }.formStyle(.grouped)
            HStack{Spacer();Button(L10n.tr("取消")){dismiss()}.keyboardShortcut(.cancelAction);Button(L10n.tr("导入 ISO")){dismiss();start(trusted)}.keyboardShortcut(.defaultAction)}
        }.padding(24).frame(width:520,height:330)
    }
}
struct ImageTechnicalFields:View {
    let image:SystemImage
    var showArguments=true
    private func field(_ label:String,_ value:String)->some View {
        Group {if !value.isEmpty {LabeledContent(L10n.tr(label)){Text(value).textSelection(.enabled).frame(maxWidth:.infinity,alignment:.leading)}}}
    }
    var body:some View {
        field("名称",image.name);field("架构",image.architecture);field("类型",image.kind);field("镜像文件",image.media ?? "");field("目录",image.root)
        if let evidence=image.bootEvidence {
            field("入口",evidence.entry);field("启动配置",evidence.config);field("内核",evidence.kernel);field("Initrd",evidence.initrd);field("安装源",evidence.sourceLabel)
            ForEach((evidence.diagnostics ?? []).filter{!["BOOT_CONFIG_FAMILY_LAYOUT","UOS_MANUAL_ONLY"].contains($0.code)},id:\.code){diagnostic in field(diagnostic.label,L10n.message(diagnostic.detail))}
        }
        if showArguments {field("参数",image.arguments)};field("启动文件映射",image.bootFiles.keys.sorted().map{"\($0)=\(image.bootFiles[$0]!)"}.joined(separator:"\n"))
    }
}
struct AboutView:View {
    var server:Server
    @Environment(\.dismiss) private var dismiss
    @State private var selectedLicense: ThirdPartyComponent?
    var body:some View {
        VStack(alignment:.leading,spacing:16){
            Text("Bootah").font(.title.bold())
            Text("\(L10n.tr("客户端版本")): \(ClientVersion.display) (\(ClientVersion.build))")
            Text("\(L10n.tr("核心版本")): \(server.connected ? server.snapshot?.coreVersion.map { "\($0.display) (\($0.build))" } ?? L10n.tr("不可用") : L10n.tr("不可用"))")
            List(ThirdPartyNotices.components.filter{$0.platforms.contains("macos")}) {entry in
                VStack(alignment:.leading,spacing:6){
                    Text(entry.name+(entry.version.isEmpty ? "" : "  "+entry.version)).font(.headline)
                    HStack(alignment:.firstTextBaseline,spacing:4){
                        Text(L10n.tr("来源")+":").fixedSize().foregroundStyle(.primary)
                        if let url=URL(string:entry.source){Link(entry.source,destination:url).textSelection(.enabled)}
                    }
                    HStack(alignment:.firstTextBaseline,spacing:4){
                        Text(L10n.tr("许可协议")+":").fixedSize().foregroundStyle(.primary)
                        if entry.notice.isEmpty,let url=URL(string:entry.licenseUrl){Link(L10n.tr(entry.license),destination:url)}
                        else {Button{selectedLicense=entry} label:{Text(L10n.tr(entry.license)).multilineTextAlignment(.leading)}.buttonStyle(.link)}
                    }
                }.padding(.vertical,8)
            }
            HStack{Spacer();Button(L10n.tr("关闭")){dismiss()}.keyboardShortcut(.defaultAction)}
        }.padding(24).frame(width:720,height:600)
        .sheet(item:$selectedLicense){entry in
            VStack(alignment:.leading,spacing:16){Text(entry.name).font(.title2);ScrollView{Text(entry.notice).font(.system(.callout,design:.monospaced)).textSelection(.enabled).frame(maxWidth:.infinity,alignment:.leading)};HStack{Spacer();Button(L10n.tr("关闭")){selectedLicense=nil}.keyboardShortcut(.defaultAction)}}.padding(24).frame(width:680,height:540)
        }
    }
}

enum FirmwareKeys {
    static let all:Set<String> = ["x86-bios","ia64","ia32-efi","x64-efi","ebc-efi","alpha","sparc64","powerpc-of","powerpc-epapr","power-opal","arm32-efi","arm64-efi","ia32-http","x64-http","arm32-http","arm64-http","arm32-uboot","arm64-uboot","riscv32-efi","riscv64-http","loongarch64-efi","riscv64-efi","hp-rmp"]
}
struct BootCommandsView:View {
    var manual:ManualBoot
    @Environment(\.dismiss) var dismiss
    @State var copied=false
    var statusText:String {
        if !manual.commands.isEmpty {return L10n.tr("在固件中执行以下命令")}
        return L10n.tr(["pxe","efi"].contains(manual.firmware) ? "从固件菜单选择网络启动，无需输入命令" : manual.firmware=="openfirmware" ? "请使用固件的网络启动入口" : "请先完成启动配置，详见下方说明")
    }
    var body:some View {
        VStack(alignment:.leading,spacing:12) {
            HStack(alignment:.firstTextBaseline) {Text(manual.title).font(.title3.bold());Spacer();Text(statusText).font(.callout).foregroundStyle(.secondary)}
            if !manual.commands.isEmpty {
                ScrollView {
                    Text(manual.commands).font(.system(.body,design:.monospaced)).textSelection(.enabled).frame(maxWidth:.infinity,alignment:.leading).padding(10)
                }.background(Color(nsColor:.textBackgroundColor)).clipShape(RoundedRectangle(cornerRadius:8)).frame(minHeight:100,maxHeight:220)
            }
            if !manual.notes.isEmpty {
                ScrollView {
                    VStack(alignment:.leading,spacing:6) {
                        ForEach(manual.notes,id:\.self) {note in Text("• "+L10n.message(note)).font(.callout).foregroundStyle(.secondary).frame(maxWidth:.infinity,alignment:.leading)}
                    }
                }.frame(maxHeight:180)
            }
            HStack {
                if !manual.commands.isEmpty {Button(copied ? L10n.t("已复制","Copied") : L10n.t("复制命令","Copy commands")) {NSPasteboard.general.clearContents();NSPasteboard.general.setString(manual.commands,forType:.string);copied=true}}
                Spacer()
                Button(L10n.t("完成","Done")) {dismiss()}.keyboardShortcut(.defaultAction)
            }
        }.padding().frame(width:640,height:460)
    }
}
struct MachineEditor: View {
    var server:Server
    @State var machine:Machine
    @Environment(\.dismiss) var dismiss
    var body:some View {
        VStack {
            Form {
                Text(L10n.tr("至少填写 MAC 或 IP")).foregroundStyle(.secondary)
                TextField("MAC",text:$machine.mac)
                TextField("IP",text:Binding(get:{machine.ip ?? ""},set:{machine.ip=$0.isEmpty ? nil : $0}))
                TextField(L10n.tr("名称"),text:$machine.name)
                TextField(L10n.tr("型号"),text:$machine.model)
                Picker(L10n.tr("启动镜像"),selection:$machine.profileId) {Text(L10n.tr("按架构自动分配")).tag(nil as String?);ForEach(server.snapshot?.profiles ?? []) {Text($0.name).tag(Optional($0.id))}}
            }.formStyle(.grouped)
            HStack {if !machine.id.isEmpty {Button(L10n.tr("删除"),role:.destructive) {Task {if await server.send("api/devices/\(machine.id)",[String:String](),method:"DELETE"){dismiss()}}}};Spacer();Button(L10n.tr("取消")){dismiss()};Button(L10n.tr("保存")){guard !machine.mac.trimmingCharacters(in:.whitespaces).isEmpty || !(machine.ip ?? "").trimmingCharacters(in:.whitespaces).isEmpty else {server.error=L10n.tr("至少填写 MAC 或 IP");return};Task {if await server.send("api/devices",machine){dismiss()}}}.keyboardShortcut(.defaultAction)}.padding()
        }.frame(width:480,height:360)
    }
}
struct ParameterInfo:Decodable {var originalArguments:String?;var editable:Bool;var applies:Bool;var preview:String}
struct ImageSettingsChange:Encodable {
    var profile:SystemImage
    var secureBoot:SecureBootChange?
}
struct ImageEditor:View {
    var server:Server
    @State var image:SystemImage
    @State var mapping=""
    @State var parameterInfo:ParameterInfo?
    @State var parameterPreview:String?
    @State var secureEnabled=false
    @State var secureMode=""
    @State var busy=false
    @Environment(\.dismiss) var dismiss
    private var secured:Bool {image.secureBoot?.enabled == true}
    private var existing:Bool {server.snapshot?.profiles.contains(where:{$0.id==image.id}) == true}
    var body:some View {
        VStack(alignment:.leading) {
            Text(L10n.tr("镜像设置")).font(.title2)
            Form {
                Section(L10n.tr("基本信息")) {
                    TextField(L10n.tr("名称"),text:$image.name)
                    if let media=image.media,!media.isEmpty {LabeledContent(L10n.tr("镜像文件")){Text(media).textSelection(.enabled)}}
                }
                if (parameterInfo?.editable ?? (!existing && image.kind != "windows-pe-iso")) && (image.kind != "windows-pe-iso" || secureEnabled) {
                Section(L10n.tr("启动参数")) {
                    TextField(L10n.tr(image.kind == "linux" ? "内核命令行" : "启动参数"),text:$image.arguments,axis:.vertical).lineLimit(3...6)
                    if image.kind == "windows" {Text(L10n.tr("Windows 参数示例：bootlog=on sos=on；多启动项使用 entry={GUID} 指定。" )).font(.caption)}
                    if existing {
                        HStack {
                            Button(L10n.tr("恢复导入时参数")){image.arguments=parameterInfo?.originalArguments ?? ""}.disabled(parameterInfo?.originalArguments == nil || parameterInfo?.editable == false)
                            Button(L10n.tr("查看参数预览")){Task {busy=true;parameterPreview=await server.parameters(image,preview:true)?.preview;busy=false}}
                        }
                        if parameterInfo != nil,parameterInfo?.originalArguments == nil {Text(L10n.tr("此镜像没有导入参数记录。")).font(.caption)}
                        if let preview=parameterPreview {ScrollView{Text(preview).font(.system(.caption,design:.monospaced)).textSelection(.enabled).frame(maxWidth:.infinity,alignment:.leading)}.frame(maxHeight:150)}
                    }
                }
                }
                if let support=image.secureBoot,support.eligible {
                    Section(L10n.tr("启动设置")) {
                        Toggle(L10n.tr("安全启动支持"),isOn:$secureEnabled)
                        if secureEnabled {
                            Picker(L10n.tr("启动方式"),selection:$secureMode) {
                                ForEach(support.modes ?? [],id:\.self){mode in Text(mode == "signed-ipxe" ? "iPXE shim" : L10n.tr("原生网络启动")).tag(mode)}
                            }
                        }
                        Text(L10n.message(support.detail)).font(.caption)
                        DisclosureGroup(L10n.tr("查看检查结果")) {
                            ForEach(support.components,id:\.file){c in Text([c.file,c.signer,c.issuer,"SHA-256: "+c.sha256,c.certificateSha256].joined(separator:"\n")).font(.caption).textSelection(.enabled)}
                        }
                    }
                }
                DisclosureGroup(L10n.tr("高级设置")) {
                    TextField(L10n.tr("架构"),text:$image.architecture).disabled(secured)
                    Picker(L10n.tr("类型"),selection:$image.kind){ForEach(["linux","bsd","windows","windows-pe-iso","windows-ris","fit","raw","solaris"],id:\.self){Text($0).tag($0)}}.disabled(secured)
                    HStack {
                        TextField(L10n.tr("目录"),text:$image.root)
                        Button(L10n.tr("选择")){let panel=NSOpenPanel();panel.canChooseDirectories=true;panel.canChooseFiles=false;if panel.runModal() == .OK {image.root=panel.url?.path ?? image.root}}
                    }.disabled(secured)
                    TextField(L10n.tr("内核 / 引导程序"),text:$image.kernel).disabled(secured)
                    TextField("Initrd",text:$image.initrd).disabled(secured)
                    TextField("DTB",text:$image.dtb).disabled(secured)
                    TextField(L10n.tr("启动文件映射"),text:$mapping,axis:.vertical).lineLimit(3...6).disabled(secured)
                }
                DisclosureGroup(L10n.tr("识别与检查结果")){ImageTechnicalFields(image:image,showArguments:parameterInfo?.editable == true)}
            }.formStyle(.grouped).disabled(busy)
            HStack {Spacer();Button(L10n.tr("取消")){dismiss()};Button(L10n.tr("保存")) {Task {
                var files:[String:String]=[:]
                for line in mapping.split(separator:"\n") {
                    let pair=line.split(separator:"=",maxSplits:1,omittingEmptySubsequences:false)
                    guard pair.count==2,!pair[0].isEmpty,!pair[1].isEmpty,files[String(pair[0])]==nil else {server.error=L10n.tr("请按 x64-efi=bootx64.efi 的格式填写，每行一项");return}
                    files[String(pair[0])]=String(pair[1])
                }
                image.bootFiles=files;busy=true
                let success:Bool
                if existing {success=await server.send("api/profiles/\(image.id)/settings",ImageSettingsChange(profile:image,secureBoot:image.secureBoot?.eligible == true ? SecureBootChange(enabled:secureEnabled,mode:secureMode):nil),method:"PUT")}
                else {success=await server.send("api/profiles",image)}
                busy=false;if success {dismiss()}
            }}.keyboardShortcut(.defaultAction)}.disabled(busy)
        }.padding(24).frame(width:650,height:660).task {
            if existing {
                busy=true
                if image.secureBoot?.eligible == true,let support=await server.secureBootSupport(image.id) {image.secureBoot=support;secureEnabled=support.enabled;secureMode=support.mode ?? support.modes?.first ?? ""}
                parameterInfo=await server.parameters(image)
                busy=false
            }
        }.onAppear {
            mapping=image.bootFiles.sorted(by:{$0.key<$1.key}).map{"\($0.key)=\($0.value)"}.joined(separator:"\n")
            secureEnabled=image.secureBoot?.enabled ?? false;secureMode=image.secureBoot?.mode ?? image.secureBoot?.modes?.first ?? ""
        }
    }
}

struct NetworkEditor:View {
    var server:Server
    @State var settings:NetworkSettings
    @State private var selection = ""
    @Environment(\.dismiss) var dismiss
    init(server:Server,settings:NetworkSettings) {
        self.server=server;_settings=State(initialValue:settings)
        _selection=State(initialValue:settings.interface.isEmpty ? "" : settings.interface+"|"+settings.serverIp)
    }
    var choices:[NetworkChoice] {
        (server.snapshot?.network?.interfaces ?? []).filter{!$0.mac.isEmpty}.flatMap { i in
            i.addresses.isEmpty ? [NetworkChoice(interface:i,address:nil)] : i.addresses.map{NetworkChoice(interface:i,address:$0)}
        }
    }
    var body:some View {
        VStack {
            Form {
                Text(server.snapshot?.network?.summary ?? "").font(.caption)
                if let detail=server.snapshot?.network?.detail,!detail.isEmpty {Text(AccessGuidance.describe(detail)).textSelection(.enabled).font(.caption).foregroundStyle(.secondary)}
                if server.snapshot?.network?.showInterfaceSelector == true {
                Picker(L10n.tr("网卡与 IPv4"),selection:$selection) {
                    Text(L10n.tr("自动选择网卡")).tag("")
                    ForEach(choices){choice in Text(choice.label).tag(choice.id)}
                    if !selection.isEmpty && !choices.contains(where:{$0.id==selection}) { Text(settings.interface).tag(selection) }
                }.onChange(of:selection){_,value in
                    if value.isEmpty { settings.interface="" }
                    else if let c=choices.first(where:{$0.id==value}) {
                        settings.interface=c.interface.id;settings.serverMac=c.interface.mac;settings.ethernetEnabled=true
                        if let a=c.address {settings.serverIp=a.ip;settings.bindIp=a.ip;settings.subnet=a.subnet}
                    }
                }
                }
                TextField(L10n.tr("服务器 IPv4"),text:$settings.serverIp)
                TextField(L10n.tr("监听 IPv4"),text:$settings.bindIp)
                TextField(L10n.tr("地址池起点"),text:$settings.poolStart)
                TextField(L10n.tr("地址池终点"),text:$settings.poolEnd)
                TextField(L10n.tr("子网掩码"),text:$settings.subnet)
                if server.snapshot?.network?.showInterfaceSelector == true { TextField(L10n.tr("网卡"),text:$settings.interface) }
                TextField(L10n.tr("网卡 MAC"),text:$settings.serverMac)
                Toggle(L10n.tr("以太网协议"),isOn:$settings.ethernetEnabled)
                Toggle(L10n.tr("自动检测 DHCP"),isOn:$settings.dhcpAuto)
                Text(L10n.tr("启用自动模式后按检测结果选择代理；不会自动启动服务")).font(.caption)
                manualModes.disabled(settings.dhcpAuto)
            }.formStyle(.grouped)
            HStack {Spacer();Button(L10n.tr("取消")){dismiss()};Button(L10n.tr("保存")){Task {
                if settings.dhcpAuto { settings.dhcpEnabled=false;settings.proxyDhcpEnabled=false }
                if await server.send("api/settings",settings,method:"PUT"){await server.detectNetwork();dismiss()}
            }}.keyboardShortcut(.defaultAction)}.padding()
        }.frame(width:620,height:620)
    }
    var manualModes:some View {
        Group {
            Toggle(L10n.t("分配地址", "Allocate addresses"),isOn:$settings.dhcpEnabled).onChange(of: settings.dhcpEnabled) { _, enabled in if enabled { settings.proxyDhcpEnabled = false } }
            Toggle(L10n.t("ProxyDHCP（外部 DHCP 分配地址）", "ProxyDHCP (external DHCP assigns addresses)"),isOn:$settings.proxyDhcpEnabled).onChange(of: settings.proxyDhcpEnabled) { _, enabled in if enabled { settings.dhcpEnabled = false } }
        }
    }
}

struct NetworkPermissionsView:View {
    @Bindable var server:Server
    @Environment(\.dismiss) var dismiss
    var body:some View {
        VStack(alignment:.leading,spacing:16){
            Text(L10n.tr("网络访问方式")).font(.title2)
            if server.checkingPermissions {ProgressView(L10n.tr("正在检查签名与公证…"))}
            else {
                if server.networkPermissions?.captureReady == true {
                    Label(L10n.tr("抓包组件已就绪"),systemImage:"checkmark.circle").font(.headline)
                }
                if server.helperAvailable {
                    Label(L10n.tr("内置网络助手可用"),systemImage:"checkmark.shield").font(.headline)
                    Text(L10n.tr("在系统设置中允许 Bootah 网络助手后，即可启动服务。"))
                    Button(L10n.tr(server.networkHelper.status == .requiresApproval ? "前往系统设置授权" : "授权检测")){Task{await server.authorizeNetworkHelper()}}.disabled(server.detecting || server.snapshot?.running == true)
                    if server.networkHelper.status == .enabled || server.networkHelper.status == .requiresApproval {Button(L10n.tr("停用网络助手")){Task{await server.disableNetworkHelper()}}.disabled(server.snapshot?.running == true)}
                } else if server.bundledCore {
                    Label(L10n.tr("管理员授权"),systemImage:"lock.shield").font(.headline)
                    Text(L10n.tr("启动服务时会请求 macOS 管理员授权。仅临时网络进程以管理员权限运行，退出应用后结束。"))
                } else {
                    Text(L10n.tr("连接外部核心时，请在服务所在主机上配置网络权限。"))
                }
            }
            if server.networkPermissions?.issues.contains(where:{$0.contains("ChmodBPF")}) == true && (server.bundledCore || server.networkPermissions?.platform == "macos") {
            Divider()
            Text(L10n.tr(server.helperAvailable ? "也可安装 ChmodBPF" : "安装 ChmodBPF")).font(.headline)
            Text(L10n.tr("1. 打开 Wireshark 官方下载页，下载 macOS 安装镜像。"))
            Text(L10n.tr("2. 打开镜像中的 Install ChmodBPF.pkg，按安装器指引授予权限。"))
            Text(L10n.tr("3. 完成后重新启动 Bootah，再重新检测或启动服务。"))
            Link(L10n.tr("打开官方下载页面"),destination:URL(string:"https://www.wireshark.org/download.html")!)
            }
            HStack{
                Button(L10n.tr("重新检查")){Task{await server.refreshNetworkPermissions()}}.disabled(server.checkingPermissions)
                Spacer();Button(L10n.tr("好")){Task{await server.confirmNetworkAccess()}}.disabled(server.starting || server.detecting || server.checkingPermissions).keyboardShortcut(.defaultAction)
            }
        }.padding(24).frame(width:540).fixedSize(horizontal:false,vertical:true)
    }
}
