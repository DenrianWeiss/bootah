using System.Net.Http;
using System.Net.Http.Json;
using System.Text.Json;
using System.Text.Json.Serialization;

namespace Netboot;
public record Device {
    public string Id {get;set;} = "";
    public string Mac {get;set;} = "";
    public string? Ip {get;set;}
    public string Name {get;set;} = "";
    public string Model {get;set;} = "";
    public string Architecture {get;set;} = "unknown";
    public string Protocol {get;set;} = "";
    public string Vendor {get;set;} = "";
    public string Uuid {get;set;} = "";
    public string? ProfileId {get;set;}
    public long LastSeen {get;set;}
    public override string ToString() => $"{(Name.Length>0?Name:Mac)}  {Ip}  {Architecture}";
}
public record Profile {
    public string Id {get;set;} = "";
    public string Name {get;set;} = "";
    public string SystemId {get;set;} = "";
    public string SystemName {get;set;} = "";
    public string Architecture {get;set;} = "x64";
    public string Kind {get;set;} = "linux";
    public string Root {get;set;} = "";
    public Dictionary<string,string> BootFiles {get;set;} = new();
    public string Kernel {get;set;} = "";
    public string Initrd {get;set;} = "";
    public string Dtb {get;set;} = "";
    public string Arguments {get;set;} = "";
    public string Media {get;set;} = "";
    public string Status {get;set;} = "";
    public string Notes {get;set;} = "";
    public SecureBootSupport? SecureBoot {get;set;}
    public BootEvidence? BootEvidence {get;set;}
    public override string ToString() => Name;
}
public record ParameterInfo(string? OriginalArguments,bool Editable,bool Applies,string Preview);
public record SecureBootSupport(bool Eligible,bool Enabled,string State,string Detail,SecureBootComponent[] Components,string? Mode=null,string[]? Modes=null);
public record SecureBootComponent(string File,string Sha256,string Signer,string Issuer,string CertificateSha256);
public record BootEvidence(string Config,string Entry,string Kernel,string Initrd,string Source,string Verification,BootDiagnostic[]? Diagnostics=null);
public record BootDiagnostic(string Code,string Detail);
public record ImportResult(Profile[] Profiles,string[] Warnings);
public record ManualBoot {
    public string Firmware {get;set;} = "";
    public string Title {get;set;} = "";
    public string Status {get;set;} = "";
    public string Commands {get;set;} = "";
    public string[] Notes {get;set;} = [];
}
public record AutomationSelection(string Method,string Filename,string ProfileId);
public record NetworkAddress(string Ip,string Subnet);
public record NetworkInterfaceInfo(string Id,string Name,string Mac,bool Up,bool Physical,NetworkAddress[] Addresses,string[] DhcpServers,string Source);
public record NetworkReport(NetworkInterfaceInfo[] Interfaces,string Selected,NetworkAddress? Address,string Status,string Source,string[] Servers,string Detail,long CheckedAt,bool ShowInterfaceSelector=false) {
    public string Summary => string.Join("  ",new[]{Strings.Tr(Status),Selected,string.Join(", ",Servers)}.Where(s=>!string.IsNullOrEmpty(s)));
}
public record NetworkChoice(NetworkInterfaceInfo? Interface,NetworkAddress? Address) {
    public override string ToString()=>Interface is null?Strings.Tr("自动选择网卡"):$"{Interface.Name}  {Address?.Ip??Strings.Tr("No IPv4")}  {Strings.Tr(Interface.Up?"已连接":"未连接网卡")}{(Interface.Physical?"":"  "+Strings.Tr("虚拟网卡"))}";
}
public record DeviceConflict(string Id,string OldId,string OldMac,string NewMac,string? Ip,string OldArchitecture,string NewArchitecture,string Reason);
public record CoreVersion(string Display,int Build);
public record Snapshot(JsonElement Settings, Device[] Devices, Profile[] Profiles, Dictionary<string,string> DefaultProfiles, JsonElement[] Events, bool Running, JsonElement[] Protocols,Dictionary<string,AutomationSelection>? Automations=null,NetworkReport? Network=null,DeviceConflict[]? DeviceConflicts=null,CoreVersion? CoreVersion=null);
public record NetworkPermissions(bool HelperAvailable,string Reason,string Platform,string[] Issues);
public class Api {
    sealed class TokenHandler : DelegatingHandler {
        public TokenHandler() : base(new HttpClientHandler{AllowAutoRedirect=false}) {}
        protected override async Task<HttpResponseMessage> SendAsync(HttpRequestMessage request,CancellationToken cancellationToken) {
            if(!string.IsNullOrEmpty(App.ApiToken))request.Headers.Authorization=new System.Net.Http.Headers.AuthenticationHeaderValue("Bearer",App.ApiToken);
            var response=await base.SendAsync(request,cancellationToken);
            if(response.IsSuccessStatusCode && (!response.Headers.TryGetValues("X-Bootah-Authentication",out var values) || !values.Contains("bearer-v1"))){response.Dispose();throw new HttpRequestException(Strings.Tr("核心未确认令牌鉴权"));}
            return response;
        }
    }
    static HttpClient CreateClient(TimeSpan timeout) {
        return new HttpClient(new TokenHandler()) {BaseAddress=new Uri(Environment.GetEnvironmentVariable("NETBOOT_API")??"http://127.0.0.1:9080"),Timeout=timeout};
    }
    readonly HttpClient client = CreateClient(TimeSpan.FromSeconds(10));
    public static readonly JsonSerializerOptions Json = new() {PropertyNamingPolicy=JsonNamingPolicy.SnakeCaseLower,PropertyNameCaseInsensitive=true};
    public async Task<Snapshot> State() => JsonSerializer.Deserialize<Snapshot>(await client.GetStringAsync("/api/state"),Json)!;
    public async Task<NetworkPermissions> Permissions(){using var requestClient=CreateClient(TimeSpan.FromSeconds(40));return JsonSerializer.Deserialize<NetworkPermissions>(await requestClient.GetStringAsync("/api/network/permissions"),Json)!;}
    public async Task Send(string path, object? body=null, HttpMethod? method=null) {
        using var request = new HttpRequestMessage(method??HttpMethod.Post,path);
        if(body!=null) request.Content=JsonContent.Create(body,options:Json);
        using var authorized=CreateClient((path.EndsWith("/compatibility")||path.Contains("/secure-boot"))?TimeSpan.FromMinutes(30):TimeSpan.FromMinutes(3));
        using var response=await ((path is "/api/service/start" or "/api/network/detect" || (path.EndsWith("/compatibility")||path.Contains("/secure-boot")))?authorized:client).SendAsync(request);
        if(!response.IsSuccessStatusCode) throw new Exception(await response.Content.ReadAsStringAsync());
    }
    public async Task<ParameterInfo> Parameters(string id,Profile? draft=null){using var request=new HttpRequestMessage(draft is null?HttpMethod.Get:HttpMethod.Post,$"/api/profiles/{id}/parameters");if(draft is not null)request.Content=JsonContent.Create(draft,options:Json);using var response=await client.SendAsync(request);var text=await response.Content.ReadAsStringAsync();if(!response.IsSuccessStatusCode)throw new Exception(text);return JsonSerializer.Deserialize<ParameterInfo>(text,Json)!;}
    public async Task<SecureBootSupport> SecureBoot(string id){using var client=CreateClient(TimeSpan.FromMinutes(3));using var response=await client.GetAsync($"/api/profiles/{id}/secure-boot");var text=await response.Content.ReadAsStringAsync();if(!response.IsSuccessStatusCode)throw new Exception(text);return JsonSerializer.Deserialize<SecureBootSupport>(text,Json)!;}
    public async Task<ImportResult> Import(string path,bool trustLocalIso=false){using var client=CreateClient(TimeSpan.FromMinutes(30));using var response=await client.PostAsJsonAsync("/api/media/import",new{path,trust_local_iso=trustLocalIso},Json);var text=await response.Content.ReadAsStringAsync();if(!response.IsSuccessStatusCode)throw new Exception(text);return JsonSerializer.Deserialize<ImportResult>(text,Json)!;}
    public Task<string> Uboot(string id)=>client.GetStringAsync($"/api/profiles/{Uri.EscapeDataString(id)}/uboot");
    public async Task<ManualBoot> BootCommands(string id,string language)=>JsonSerializer.Deserialize<ManualBoot>(await client.GetStringAsync($"/api/profiles/{Uri.EscapeDataString(id)}/boot-commands?lang={Uri.EscapeDataString(language)}"),Json)!;
    public async Task<string[]> AutomationMethods(string id)=>JsonSerializer.Deserialize<string[]>(await client.GetStringAsync($"/api/profiles/{Uri.EscapeDataString(id)}/automation-methods"),Json)??[];
    public async Task SetAutomation(string scope,string id,string method,string path){using var request=new HttpRequestMessage(HttpMethod.Put,$"/api/automation/{scope}/{Uri.EscapeDataString(id)}?method={Uri.EscapeDataString(method)}&filename={Uri.EscapeDataString(Path.GetFileName(path))}"){Content=new ByteArrayContent(await File.ReadAllBytesAsync(path))};using var response=await client.SendAsync(request);if(!response.IsSuccessStatusCode)throw new Exception(await response.Content.ReadAsStringAsync());}
    public Task ClearAutomation(string scope,string id)=>Send($"/api/automation/{scope}/{Uri.EscapeDataString(id)}",method:HttpMethod.Delete);
}
