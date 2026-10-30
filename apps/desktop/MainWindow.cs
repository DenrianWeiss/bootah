using Avalonia;
using Avalonia.Controls;
using Avalonia.Layout;
using Avalonia.Threading;
using Avalonia.Styling;
using Avalonia.Media.Imaging;

namespace Netboot;
public class MainWindow : Window {
    readonly Api api=new();
    readonly CheckBox trustLocalIso=new(){IsChecked=false};
    readonly ListBox devices=new(), systems=new(), events=new();
    readonly StackPanel architectureDefaults=new(){Spacing=8};
    readonly ComboBox logLevel=new(){SelectedIndex=1,Width=130};
    readonly CheckBox includePrivate=new();
    readonly TextBlock status=new() {Text=Strings.Tr("未连接"),VerticalAlignment=VerticalAlignment.Center,MaxWidth=150,TextWrapping=Avalonia.Media.TextWrapping.Wrap};
    readonly Button service=new(){Content=Strings.Tr("启动")};
    readonly DispatcherTimer timer=new(){Interval=TimeSpan.FromSeconds(3)};
    Snapshot? current;
    bool refreshing;
    bool networkChecked;
    bool checkingNetwork;
    NetworkPermissions? permissions;
    string? networkCheckError;
    readonly Dictionary<string,(string Signature,string[] Methods)> automationCapabilities=new();
    readonly List<(Border Host,Button Button,string Scope)> automationButtons=new();
    readonly List<(Button Button,string Scope)> deleteButtons=new();
    readonly TextBlock networkStatus=new(){TextWrapping=Avalonia.Media.TextWrapping.Wrap,FontSize=12};
    static bool Chinese=>System.Globalization.CultureInfo.CurrentUICulture.TwoLetterISOLanguageName.Equals("zh",StringComparison.OrdinalIgnoreCase);
    static string T(string chinese,string english){var value=Strings.Tr(chinese);return !Chinese&&value==chinese?english:value;}
    static string Tr(string chinese)=>Strings.Tr(chinese);
    readonly Dictionary<string,Button> navigation=new();
    bool Dark=>ActualThemeVariant==Avalonia.Styling.ThemeVariant.Dark;
    Avalonia.Media.IBrush Tone(string light,string dark)=>new Avalonia.Media.SolidColorBrush(Avalonia.Media.Color.Parse(Dark?dark:light));
    readonly TextBlock pageTitle=new(){FontSize=28,FontWeight=Avalonia.Media.FontWeight.SemiBold};
    readonly ContentControl body=new();
    readonly WrapPanel tools=new(){Orientation=Orientation.Horizontal};
    public MainWindow() {
        Icon=new WindowIcon(Avalonia.Platform.AssetLoader.Open(new Uri("avares://Bootah/Assets/bootah.ico")));
        Title="Bootah";Width=1120;Height=740;MinWidth=800;MinHeight=520;
        Background=Tone("#F8FAFC","#1B1E24");
        foreach(var list in new[]{devices,systems,events}){list.Background=Avalonia.Media.Brushes.Transparent;list.BorderThickness=new Thickness(0);list.Styles.Add(new Avalonia.Styling.Style(x=>x.OfType<ListBoxItem>()){Setters={new Avalonia.Styling.Setter(ListBoxItem.HorizontalContentAlignmentProperty,HorizontalAlignment.Stretch)}});}
        devices.ItemTemplate=new Avalonia.Controls.Templates.FuncDataTemplate<Device>((d,_)=>DeviceRow(d));
        systems.ItemTemplate=new Avalonia.Controls.Templates.FuncDataTemplate<Profile>((p,_)=>Row(string.IsNullOrWhiteSpace(p?.SystemName)?p?.Name??"":p.SystemName,
            string.Join("  ",new[]{p?.Name,p?.Architecture,p?.Kind}.Where(x=>!string.IsNullOrWhiteSpace(x))),
            p?.Status=="needs-attention"?Tr("待配置"):"",LogoId(p)));
        devices.DoubleTapped+=async(_,_)=>await Run(()=>EditDevice(devices.SelectedItem as Device));
        devices.SelectionChanged+=(_,_)=>UpdateAutomationButtons();systems.SelectionChanged+=(_,_)=>UpdateAutomationButtons();
        systems.DoubleTapped+=async(_,_)=>await Run(()=>EditProfile(systems.SelectedItem as Profile));
        var rail=new StackPanel{Spacing=8,Margin=new Thickness(16,24)};
        rail.Children.Add(new TextBlock{Text="Bootah",FontSize=23,FontWeight=Avalonia.Media.FontWeight.SemiBold,Margin=new Thickness(12,0,0,32)});
        foreach(var name in new[]{"设备","镜像库","活动"}){var button=Action(PageName(name),()=>{ShowPage(name);return Task.CompletedTask;});button.HorizontalAlignment=HorizontalAlignment.Stretch;button.HorizontalContentAlignment=HorizontalAlignment.Left;button.Padding=new Thickness(16,12);button.Background=Avalonia.Media.Brushes.Transparent;button.BorderThickness=new Thickness(0);navigation[name]=button;rail.Children.Add(button);}
        var side=new DockPanel{LastChildFill=false};DockPanel.SetDock(rail,Dock.Top);side.Children.Add(rail);
        var connection=new StackPanel{Spacing=12,Margin=new Thickness(28),Children={status,networkStatus,Action(T("网络设置","Network settings"),Network)}};DockPanel.SetDock(connection,Dock.Bottom);side.Children.Add(connection);
        if(Environment.GetEnvironmentVariable("NETBOOT_API") is not null)connection.Children.Add(Action(Tr("管理令牌"),EditToken));
        var main=new DockPanel{Margin=new Thickness(32,26)};
        var header=new DockPanel{Margin=new Thickness(0,0,0,24)};
        service.Classes.Add("accent");service.Padding=new Thickness(22,10);DockPanel.SetDock(service,Dock.Right);header.Children.Add(service);var about=Action(T("关于与开源许可","About & open-source licenses"),About);DockPanel.SetDock(about,Dock.Right);header.Children.Add(about);header.Children.Add(pageTitle);DockPanel.SetDock(header,Dock.Top);main.Children.Add(header);
        tools.Margin=new Thickness(0,0,0,18);DockPanel.SetDock(tools,Dock.Top);main.Children.Add(tools);main.Children.Add(body);
        var grid=new Grid{ColumnDefinitions=new ColumnDefinitions("220,*")};grid.Children.Add(new Border{Background=Tone("#EEF2F7","#22262E"),Child=side});Grid.SetColumn(main,1);grid.Children.Add(main);Content=grid;
        logLevel.ItemsSource=new[]{T("错误","Errors"),T("常规","Standard"),T("详细","Detailed")};
        includePrivate.Content=T("包含原始隐私信息","Include private data");
        ShowPage("设备");
        service.Click+=async (_,_)=>await Run(async()=>{service.IsEnabled=false;try{await api.Send(current?.Running==true?"/api/service/stop":"/api/service/start");await Refresh();}finally{service.IsEnabled=true;}});
        timer.Tick+=async (_,_)=>await Refresh();Opened+=async (_,_)=>{if(Environment.GetEnvironmentVariable("NETBOOT_API") is not null && string.IsNullOrEmpty(App.ApiToken))await EditToken();await Refresh();timer.Start();};Closed+=(_,_)=>timer.Stop();
    }
    static readonly HashSet<string> knownLogos=["ubuntu","debian","fedora","archlinux","linuxmint","nixos","alpinelinux","opensuse","centos","redhat","rockylinux","almalinux","linux","aosc","freebsd","netbsd","openbsd","solaris","windowsxp","windowsserver2003"];
    static string LogoId(Profile? p){var id=p?.SystemId switch {"windows11" or "windows"=>"windows11-original","windows10" or "windows8" or "windows7" or "windowsserver"=>"windows8-original","kylin" or "uos"=>"linux",null or "" when p?.Kind=="linux"=>"linux",_=>p?.SystemId??""};return knownLogos.Contains(id)||(id is "windows11-original" or "windows8-original")?id:"";}
    static readonly Dictionary<string,Bitmap> logos=new();
    Control Row(string title,string subtitle,string badge,string? logoId=null){var row=new DockPanel{Margin=new Thickness(8,12)};if(badge.Length>0){var label=new TextBlock{Text=badge,MaxWidth=260,TextTrimming=Avalonia.Media.TextTrimming.CharacterEllipsis,VerticalAlignment=VerticalAlignment.Center,Opacity=.7,Margin=new Thickness(24,0,0,0)};DockPanel.SetDock(label,Dock.Right);row.Children.Add(label);}if(!string.IsNullOrWhiteSpace(logoId)){var path=Path.Combine(AppContext.BaseDirectory,"system-icons",Dark?"dark":"light",logoId+".png");if(File.Exists(path)){if(!logos.TryGetValue(path,out var bitmap)){bitmap=new Bitmap(path);logos[path]=bitmap;}var icon=new Border{Width=42,Height=42,Padding=new Thickness(6),CornerRadius=new CornerRadius(8),Margin=new Thickness(0,0,12,0),Child=new Image{Source=bitmap,Stretch=Avalonia.Media.Stretch.Uniform}};DockPanel.SetDock(icon,Dock.Left);row.Children.Add(icon);}}row.Children.Add(new StackPanel{Spacing=5,Children={new TextBlock{Text=title,FontSize=16,FontWeight=Avalonia.Media.FontWeight.SemiBold},new TextBlock{Text=subtitle,FontSize=12,Opacity=.65}}});return new Border{Child=row,Padding=new Thickness(12,2),CornerRadius=new CornerRadius(10),Background=Tone("#FFFFFF","#282D36"),Margin=new Thickness(0,0,0,8)};}
    Control DeviceRow(Device? device){
        if(device is null)return new TextBlock();
        var automatic=current?.DefaultProfiles.GetValueOrDefault(device.Architecture);
        var automaticName=current?.Profiles.FirstOrDefault(p=>p.Id==automatic)?.Name;
        var options=new[]{new Profile{Name=automaticName is null?Tr("未分配"):$"{Tr("按架构自动分配")}  {automaticName}"}}.Concat(current?.Profiles??Array.Empty<Profile>()).ToArray();
        var selector=new ComboBox{ItemsSource=options,SelectedIndex=Math.Max(0,Array.FindIndex(options,p=>p.Id==device.ProfileId)),MinWidth=190,MaxWidth=260,HorizontalAlignment=HorizontalAlignment.Right};
        selector.SelectionChanged+=async (_,_)=>{if(selector.SelectedItem is not Profile selected||selected.Id==device.ProfileId)return;await Run(async()=>{var changed=device with{ProfileId=string.IsNullOrEmpty(selected.Id)?null:selected.Id};await api.Send("/api/devices",changed);await Refresh();});};
        var row=new Grid{ColumnDefinitions=new ColumnDefinitions("*,Auto"),Margin=new Thickness(20,14)};
        row.Children.Add(new StackPanel{Spacing=5,Children={new TextBlock{Text=string.IsNullOrWhiteSpace(device.Name)?(string.IsNullOrWhiteSpace(device.Mac)?device.Ip??Tr("IP 绑定"):device.Mac):device.Name,FontSize=16,FontWeight=Avalonia.Media.FontWeight.SemiBold},new TextBlock{Text=string.Join("  ",new[]{device.Ip,device.Architecture,device.Model}.Where(x=>!string.IsNullOrWhiteSpace(x))),FontSize=12,Opacity=.65}}});
        Grid.SetColumn(selector,1);row.Children.Add(selector);
        return new Border{Child=row,CornerRadius=new CornerRadius(10),Background=Tone("#FFFFFF","#282D36"),Margin=new Thickness(0,0,0,8)};
    }
    static string PageName(string name)=>name switch{"设备"=>T("设备与 Profile","Devices & profiles"),"镜像库"=>T("介质与导入","Media & import"),_=>T("传输与日志","Transfers & logs")};
    void ShowPage(string name){foreach(var pair in navigation){pair.Value.Background=pair.Key==name?Tone("#DCE8F8","#33445D"):Avalonia.Media.Brushes.Transparent;}pageTitle.Text=PageName(name);tools.Children.Clear();automationButtons.Clear();deleteButtons.Clear();if(name=="设备"){body.Content=devices;tools.Children.Add(Action(T("添加设备","Add device"),()=>EditDevice(null)));tools.Children.Add(Action(T("分配镜像","Assign image"),()=>EditDevice(devices.SelectedItem as Device)));tools.Children.Add(AutomationButton("device"));tools.Children.Add(DeleteButton("device"));}else if(name=="镜像库"){var mediaLayout=new DockPanel();var defaultsScroll=new ScrollViewer{Content=architectureDefaults,MaxHeight=240,Margin=new Thickness(0,0,0,16)};DockPanel.SetDock(defaultsScroll,Dock.Top);mediaLayout.Children.Add(defaultsScroll);mediaLayout.Children.Add(systems);body.Content=mediaLayout;RenderArchitectureDefaults();var import=Action(T("导入 ISO","Import ISO"),ImportIso);import.Classes.Add("accent");tools.Children.Add(import);trustLocalIso.Content=T("信任所选本地 ISO 安装源","Trust selected local ISO source");tools.Children.Add(trustLocalIso);tools.Children.Add(Action(T("手动添加","Add manually"),()=>EditProfile(null)));tools.Children.Add(Action(T("编辑","Edit"),()=>EditProfile(systems.SelectedItem as Profile)));tools.Children.Add(Action(Tr("镜像详情"),()=>ShowImageDetails(systems.SelectedItem as Profile)));tools.Children.Add(AutomationButton("profile"));tools.Children.Add(Action(Tr("启动方式"),Commands));tools.Children.Add(DeleteButton("profile"));}else{body.Content=events;tools.Children.Add(logLevel);tools.Children.Add(includePrivate);tools.Children.Add(Action(Tr("清除日志"),async()=>{await api.Send("/api/events",new{},HttpMethod.Delete);await Refresh();}));tools.Children.Add(Action(Tr("回到日志顶部"),()=>{if(events.ItemCount>0 && events.Items[0] is object first)events.ScrollIntoView(first);return Task.CompletedTask;}));tools.Children.Add(Action(T("导出诊断日志","Export diagnostics"),ExportDiagnostics));}foreach(var control in tools.Children){control.Margin=new Thickness(0,0,name=="活动"?20:8,8);control.VerticalAlignment=Avalonia.Layout.VerticalAlignment.Center;}UpdateAutomationButtons();}
    async Task ExportDiagnostics(){
        if(!File.Exists(App.CoreBinary))throw new Exception(T("找不到本机核心程序","Local core executable not found"));
        if(includePrivate.IsChecked==true){var confirm=new FormWindow(T("原始日志包含隐私信息","Raw logs contain private data"));confirm.Note(T("原始日志可能包含设备地址和本地文件路径。","Raw logs may contain device addresses and local file paths."));if(!await confirm.ShowDialog<bool>(this))return;}
        var picked=await StorageProvider.SaveFilePickerAsync(new Avalonia.Platform.Storage.FilePickerSaveOptions{Title=T("导出诊断日志","Export diagnostics"),SuggestedFileName="bootah-diagnostics.txt"});if(picked is null)return;
        var start=new System.Diagnostics.ProcessStartInfo(App.CoreBinary){UseShellExecute=false,CreateNoWindow=true,RedirectStandardOutput=true,RedirectStandardError=true};
        foreach(var value in new[]{"--export-diagnostics","--config",App.CoreConfig,"--log-level",new[]{"error","info","debug"}[Math.Clamp(logLevel.SelectedIndex,0,2)]})start.ArgumentList.Add(value);
        if(includePrivate.IsChecked==true)start.ArgumentList.Add("--include-private");
        using var process=System.Diagnostics.Process.Start(start)??throw new Exception(T("无法启动日志导出","Could not start diagnostics export"));
        var output=process.StandardOutput.ReadToEndAsync();var error=process.StandardError.ReadToEndAsync();await process.WaitForExitAsync();
        if(process.ExitCode!=0)throw new Exception(await error);
        await using var stream=await picked.OpenWriteAsync();var bytes=System.Text.Encoding.UTF8.GetBytes(await output);await stream.WriteAsync(bytes);await stream.FlushAsync();
    }
    Profile? AutomationProfile(string scope,string? id)=>scope=="profile"?current?.Profiles.FirstOrDefault(p=>p.Id==id):current?.Devices.FirstOrDefault(d=>d.Id==id) is Device d?current?.Profiles.FirstOrDefault(p=>p.Id==(d.ProfileId??current.DefaultProfiles.GetValueOrDefault(d.Architecture))):null;
    Control AutomationButton(string scope){
        var button=Action(Tr("自动安装配置"),()=>ConfigureAutomation(scope,scope=="profile"?(systems.SelectedItem as Profile)?.Id:(devices.SelectedItem as Device)?.Id));
        var host=new Border{Child=button,Background=Avalonia.Media.Brushes.Transparent};automationButtons.Add((host,button,scope));return host;
    }
    Button DeleteButton(string scope){
        var button=Action(Tr("删除"),scope=="device"?RemoveDevice:RemoveProfile);
        deleteButtons.Add((button,scope));return button;
    }
    void UpdateAutomationButtons(){
        foreach(var (button,scope) in deleteButtons)button.IsEnabled=scope=="device"?devices.SelectedItem is Device:systems.SelectedItem is Profile;
        foreach(var (host,button,scope) in automationButtons){
            var id=scope=="profile"?(systems.SelectedItem as Profile)?.Id:(devices.SelectedItem as Device)?.Id;
            var p=AutomationProfile(scope,id);
            var loaded=p is not null&&automationCapabilities.TryGetValue(p.Id,out var capability)&&capability.Signature==System.Text.Json.JsonSerializer.Serialize(p,Api.Json);
            button.IsEnabled=loaded&&automationCapabilities[p!.Id].Methods.Length>0;
            ToolTip.SetTip(host,button.IsEnabled?null:Tr(id is null?"请先选择设备或镜像":p is null?"请先分配启动镜像":loaded?"该平台不支持自动配置":"正在读取自动配置选项…"));
        }
    }
    async Task LoadAutomationCapabilities(){
        if(current is null)return;
        foreach(var p in current.Profiles){var signature=System.Text.Json.JsonSerializer.Serialize(p,Api.Json);if(automationCapabilities.TryGetValue(p.Id,out var cached)&&cached.Signature==signature)continue;automationCapabilities[p.Id]=(signature,await api.AutomationMethods(p.Id));}
        UpdateAutomationButtons();
    }
    async Task ConfigureAutomation(string scope,string? id){if(current is null||string.IsNullOrEmpty(id))return;var profile=scope=="profile"?current.Profiles.FirstOrDefault(p=>p.Id==id):current.Devices.FirstOrDefault(d=>d.Id==id) is Device d?current.Profiles.FirstOrDefault(p=>p.Id==(d.ProfileId??current.DefaultProfiles.GetValueOrDefault(d.Architecture))):null;if(profile is null)return;var methods=await api.AutomationMethods(profile.Id);if(methods.Length==0)return;var existing=current.Automations?.GetValueOrDefault($"{scope}:{id}");var choices=new[]{T("不使用自动配置","No automatic configuration")}.Concat(methods).ToArray();var form=new FormWindow(T("自动安装配置","Automatic installation"));form.Note($"{profile.Name}  {existing?.Filename??T("未设置","Not configured")}");var selected=form.Choose(T("配置机制","Configuration method"),choices,Math.Max(0,Array.IndexOf(choices,existing?.Method)));if(!await form.ShowDialog<bool>(this))return;var index=selected.SelectedIndex;if(index<=0){await api.ClearAutomation(scope,id);await Refresh();return;}var picked=await StorageProvider.OpenFilePickerAsync(new Avalonia.Platform.Storage.FilePickerOpenOptions{Title=T("选择配置文件","Select configuration file"),AllowMultiple=false});if(picked.Count==0)return;await api.SetAutomation(scope,id,methods[index-1],picked[0].Path.LocalPath);await Refresh();}
    void RenderArchitectureDefaults(){
        architectureDefaults.Children.Clear();
        architectureDefaults.Children.Add(new TextBlock{Text=Tr("架构默认启动镜像"),FontSize=16});
        if(current is null)return;
        var rows=new List<Control>();
        foreach(var key in current.Profiles.SelectMany(p=>p.BootFiles.Keys).Where(FirmwareKeys.Contains).Distinct().OrderBy(x=>x)){
            var name=current.Profiles.FirstOrDefault(p=>p.Id==current.DefaultProfiles.GetValueOrDefault(key))?.Name??Tr("未设置");
            var row=new Grid{ColumnDefinitions=new ColumnDefinitions("170,*"),Margin=new Thickness(0,4)};
            row.Children.Add(new TextBlock{Text=key,VerticalAlignment=VerticalAlignment.Center});
            var change=Action(name,()=>EditArchitectureDefault(key));change.HorizontalAlignment=HorizontalAlignment.Stretch;
            change.Content=new TextBlock{Text=name,TextTrimming=Avalonia.Media.TextTrimming.CharacterEllipsis,TextWrapping=Avalonia.Media.TextWrapping.NoWrap,MaxWidth=480};
            ToolTip.SetTip(change,name);Grid.SetColumn(change,1);row.Children.Add(change);rows.Add(row);
        }
        architectureDefaults.Children.Add(new ListBox{ItemsSource=rows});
    }
    async Task EditArchitectureDefault(string key){
        if(current is null)return;
        var options=new[]{new Profile{Name=Tr("未设置")}}.Concat(current.Profiles.Where(p=>p.BootFiles.ContainsKey(key))).ToArray();
        var form=new FormWindow(key);
        var choice=form.Choose(Tr("启动镜像"),options.Select(p=>p.Name).ToArray(),Math.Max(0,Array.FindIndex(options,p=>p.Id==current.DefaultProfiles.GetValueOrDefault(key))));
        choice.MaxWidth=440;choice.ItemTemplate=new Avalonia.Controls.Templates.FuncDataTemplate<string>((name,_)=>{
            var label=new TextBlock{Text=name,TextTrimming=Avalonia.Media.TextTrimming.CharacterEllipsis,TextWrapping=Avalonia.Media.TextWrapping.NoWrap,MaxWidth=420};
            ToolTip.SetTip(label,name);return label;
        });
        if(!await form.ShowDialog<bool>(this)||current is null)return;
        var defaults=new Dictionary<string,string>(current.DefaultProfiles);var id=options[choice.SelectedIndex].Id;
        if(string.IsNullOrEmpty(id))defaults.Remove(key);else defaults[key]=id;
        await api.Send("/api/default-profiles",defaults,HttpMethod.Put);await Refresh();
    }
    static readonly HashSet<string> FirmwareKeys=["x86-bios","ia64","ia32-efi","x64-efi","ebc-efi","alpha","powerpc-of","powerpc-epapr","power-opal","arm32-efi","arm64-efi","ia32-http","x64-http","arm32-http","arm64-http","arm32-uboot","arm64-uboot","riscv32-efi","riscv64-http","loongarch64-efi","riscv64-efi","hp-rmp"];
    async Task ImportIso(){
        var trusted=trustLocalIso.IsChecked==true;trustLocalIso.IsChecked=false;
        var picked=await StorageProvider.OpenFilePickerAsync(new Avalonia.Platform.Storage.FilePickerOpenOptions{Title=Tr("导入 ISO"),AllowMultiple=false,FileTypeFilter=new[]{new Avalonia.Platform.Storage.FilePickerFileType("ISO"){Patterns=new[]{"*.iso"}}}});
        if(picked.Count==0)return;status.Text=Tr("正在导入…");tools.IsEnabled=false;
        try{var result=await api.Import(picked[0].Path.LocalPath,trusted);await Refresh();status.Text=Tr("ISO 导入完成");var issues=result.Warnings;if(issues.Length>0){var form=new FormWindow(Tr("需要处理的问题"));foreach(var issue in issues)form.Note(Strings.Message(issue));await form.ShowDialog<bool>(this);}}
        finally{tools.IsEnabled=true;}
    }

    static bool ActionableDiagnostic(BootDiagnostic d)=>d.Code is not ("BOOT_CONFIG_FAMILY_LAYOUT" or "UOS_MANUAL_ONLY");
    async Task ShowImageDetails(Profile? p){
        if(p is null)return;var form=new FormWindow(Tr("镜像详情"));
        void Field(string label,string value,bool multiline=false){if(!string.IsNullOrWhiteSpace(value))form.Text(Tr(label),value,multiline).IsReadOnly=true;}
        if(p.SecureBoot is {Eligible:true} support){
            form.Section(T("启动设置","Boot settings"));
            var toggle=form.Check(T("安全启动支持","Secure Boot support"),support.Enabled);
            var detail=new TextBlock{Text=Strings.Message(support.Detail),TextWrapping=Avalonia.Media.TextWrapping.Wrap};form.Add(detail);
            var results=form.Text(T("查看检查结果","View check results"),"",true);results.IsReadOnly=true;
            bool updating=false,loading=true;
            void Render(){updating=true;toggle.IsChecked=support.Enabled;toggle.IsEnabled=!loading&&(support.Enabled||support.State=="passed");detail.Text=loading?T("正在准备或检查…","Preparing or checking…"):Strings.Message(support.Detail);results.Text=string.Join("\n\n",support.Components.Select(c=>$"{c.File}\n{c.Signer}\n{c.Issuer}\nSHA-256: {c.Sha256}\n{c.CertificateSha256}"));updating=false;}
            async Task Load(){loading=true;Render();try{support=await api.SecureBoot(p.Id);}catch(Exception e){status.Text=Strings.Message(e.Message);}finally{loading=false;Render();}}
            async Task Change(){var enabled=toggle.IsChecked==true;loading=true;Render();try{await api.Send($"/api/profiles/{p.Id}/secure-boot",new{enabled},HttpMethod.Put);await Refresh();}catch(Exception e){status.Text=Strings.Message(e.Message);}finally{await Load();}}
            Render();form.Opened+=async(_,_)=>await Load();toggle.IsCheckedChanged+=async(_,_)=>{if(!updating)await Change();};
        }
        Field("名称",p.Name);Field("架构",p.Architecture);Field("类型",p.Kind);Field("目录",p.Root);Field("镜像文件",p.Media);
        if(p.BootEvidence is BootEvidence e){Field("入口",e.Entry);Field("启动配置",e.Config);Field("内核",e.Kernel);Field("Initrd",e.Initrd);Field("安装源",SourceLabel(e.Source));foreach(var d in (e.Diagnostics??[]).Where(ActionableDiagnostic))Field(d.Code is "SOURCE_REQUIRED" or "SOURCE_UNVERIFIED"?"需要配置安装源":d.Code=="SOURCE_STAGE2_MISSING"?"缺少第二阶段文件":d.Code=="SOURCE_REPOSITORY_MISSING"?"未找到软件仓库索引":"需要处理的问题",Strings.Message(d.Detail));}
        Field("参数",p.Arguments,true);Field("启动文件映射",string.Join("\n",p.BootFiles.Select(pair=>$"{pair.Key}={pair.Value}")),true);await form.ShowDialog<bool>(this);
    }
    static string SourceLabel(string source)=>source switch{"http-tree"=>Tr("HTTP 安装树"),"http-iso"=>Tr("HTTP 原盘"),"nfs-tree"=>Tr("只读 NFS 文件树"),"nbd"=>Tr("只读 NBD 原盘"),"external"=>Tr("外部安装源"),"local-media"=>Tr("需要本地介质"),_=>Tr("未识别安装源")};
    Button Action(string name,Func<Task> action) {var b=new Button{Content=name};b.Click+=async (_,_)=>await Run(action);return b;}
    async Task CheckNetwork(bool authorize) {
        if(checkingNetwork)return;
        checkingNetwork=true;networkCheckError=null;permissions=null;UpdateNetworkStatus();
        try {
            current=await api.State();
            if(!current.Running){
                await api.Send("/api/network/detect",new{});
                current=await api.State();
                if(authorize && current.Network?.Status=="probe_unavailable" && CanAuthorize(current.Network.Detail)){
                    await api.Send("/api/network/detect",new{authorize=true});
                    current=await api.State();
                }
            }
            permissions=await api.Permissions();
        }catch(Exception e){networkCheckError=AccessGuidance.Describe(e.Message);}
        finally{checkingNetwork=false;}
        UpdateNetworkStatus();
    }
    static bool CanAuthorize(string detail)=>!detail.StartsWith("Install ",StringComparison.OrdinalIgnoreCase)
        && !detail.Contains("could not be loaded",StringComparison.OrdinalIgnoreCase)
        && new[]{"permission","permitted","denied","authorization"}.Any(word=>detail.Contains(word,StringComparison.OrdinalIgnoreCase));
    string DhcpStatus()=>current?.Network?.Status switch {
        "lease_found"=>T("已有 DHCP 记录","DHCP record found"),
        "response_found"=>T("已发现 DHCP 服务器","DHCP server found"),
        "probe_unavailable"=>T("无法检测","Check unavailable"),
        null=>T("未检查","Not checked"),
        var value=>Tr(value)
    };
    string[] NetworkIssues()=>new[]{networkCheckError,current?.Network?.Detail}
        .Concat(permissions?.Issues??[]).Where(x=>!string.IsNullOrWhiteSpace(x)).Select(x=>AccessGuidance.Describe(x!)).Distinct().ToArray();
    void UpdateNetworkStatus(){
        networkStatus.Text=checkingNetwork?Tr("检测中…"):networkCheckError is not null?T("检查失败","Check failed"):
            NetworkIssues().Length>0?T("网络需要处理","Network needs attention"):DhcpStatus();
        ToolTip.SetTip(networkStatus,null);
    }
    internal async Task ShowFailure(string message){
        message=Strings.Tr(message);
        var window=new Window{Title="Bootah",Width=620,Height=330,WindowStartupLocation=WindowStartupLocation.CenterOwner};
        var panel=new DockPanel{Margin=new Thickness(20)};
        var close=new Button{Content=Tr("好"),HorizontalAlignment=HorizontalAlignment.Right,Margin=new Thickness(0,16,0,0)};
        close.Click+=(_,_)=>window.Close();DockPanel.SetDock(close,Dock.Bottom);panel.Children.Add(close);
        panel.Children.Add(new ScrollViewer{Content=new SelectableTextBlock{Text=AccessGuidance.Describe(message),TextWrapping=Avalonia.Media.TextWrapping.Wrap}});
        if(AccessGuidance.DownloadUrl(message) is string url){
            var download=new Button{Content=T("打开官方下载页面","Open official download page"),Margin=new Thickness(0,12,0,0)};
            download.Click+=(_,_)=>{try{System.Diagnostics.Process.Start(new System.Diagnostics.ProcessStartInfo(url){UseShellExecute=true});}catch(Exception e){download.Content=e.Message;}};
            DockPanel.SetDock(download,Dock.Bottom);panel.Children.Add(download);
        }
        window.Content=panel;await window.ShowDialog(this);
    }
    async Task Run(Func<Task> action){try{await action();}catch(Exception e){await ShowFailure(e.Message);}}
    async Task Refresh(){if(refreshing||checkingNetwork)return;refreshing=true;try{current=await api.State();RenderArchitectureDefaults();UpdateNetworkStatus();var id=(devices.SelectedItem as Device)?.Id;var pid=(systems.SelectedItem as Profile)?.Id;devices.ItemsSource=current.Devices;systems.ItemsSource=current.Profiles;devices.SelectedItem=current.Devices.FirstOrDefault(d=>d.Id==id);systems.SelectedItem=current.Profiles.FirstOrDefault(p=>p.Id==pid);events.ItemsSource=current.Events.Reverse().Take(100).Select(e=>$"{DateTimeOffset.FromUnixTimeSeconds(e.GetProperty("time").GetInt64()).ToLocalTime():HH:mm:ss}  {e.GetProperty("device").GetString()}  {e.GetProperty("protocol").GetString()}  {e.GetProperty("message").GetString()}").ToArray();status.Text=Tr(current.Running?"运行中":"已停止");service.Content=Tr(current.Running?"停止":"启动");await LoadAutomationCapabilities();if(!networkChecked){networkChecked=true;await CheckNetwork(true);if(NetworkIssues().Length>0)await Network();}if(current.DeviceConflicts?.FirstOrDefault() is DeviceConflict conflict)await ConfirmDeviceConflict(conflict);}catch(Exception e){status.Text=$"{Tr("未连接")}  {Strings.Tr(e.Message)}";}finally{refreshing=false;}}
    async Task ConfirmDeviceConflict(DeviceConflict conflict){
        var details=conflict.Reason=="ip-mac"?$"IP {conflict.Ip}: {conflict.OldMac} → {conflict.NewMac}":$"{conflict.NewMac}: {conflict.OldArchitecture} → {conflict.NewArchitecture}";
        var form=new FormWindow(Tr("清理旧设备记录？"));form.Note(details);form.Note(Tr("旧设备有手动配置。清理后，新设备将在下次网络启动时使用默认镜像。"));
        var accept=await form.ShowDialog<bool>(this);
        await api.Send($"/api/device-conflicts/{Uri.EscapeDataString(conflict.Id)}",new{accept});
    }
    async Task EditToken(){var form=new FormWindow(Tr("管理令牌"));var input=form.Text(Tr("管理令牌"),App.ApiToken??"");input.PasswordChar='●';var save=form.Check(Tr("安全保存令牌"),true);if(await form.ShowDialog<bool>(this)){App.ApiToken=input.Text?.Trim();var endpoint=Environment.GetEnvironmentVariable("NETBOOT_API")!;if(save.IsChecked==true&&!string.IsNullOrEmpty(App.ApiToken)){if(!TokenStore.Save(endpoint,App.ApiToken)){TokenStore.Remove(endpoint);await ShowFailure(Tr("无法安全保存令牌"));}}else TokenStore.Remove(endpoint);await Refresh();}}
    async Task EditDevice(Device? source){if(current==null)return;var d=source is null?new Device():source with{};var form=new FormWindow(Tr("设备"));form.Note(Tr("至少填写 MAC 或 IP"));var mac=form.Text("MAC",d.Mac);var ip=form.Text("IP",d.Ip??"");var name=form.Text(Tr("名称"),d.Name);var model=form.Text(Tr("型号"),d.Model);var options=new[]{new Profile{Name=Tr("未分配")}}.Concat(current.Profiles).ToArray();var profiles=form.Choose(Tr("启动镜像"),options,Array.FindIndex(options,p=>p.Id==d.ProfileId));if(await form.ShowDialog<bool>(this)){if(string.IsNullOrWhiteSpace(mac.Text)&&string.IsNullOrWhiteSpace(ip.Text))throw new Exception(Tr("至少填写 MAC 或 IP"));d.Name=name.Text??"";d.Mac=mac.Text?.Trim()??"";d.Ip=string.IsNullOrWhiteSpace(ip.Text)?null:ip.Text.Trim();d.Model=model.Text??"";d.ProfileId=(profiles.SelectedItem as Profile)?.Id;if(d.ProfileId=="")d.ProfileId=null;await api.Send("/api/devices",d);await Refresh();}}
    async Task EditProfile(Profile? source){var p=source is null?new Profile{Id=Guid.NewGuid().ToString("N")}:source with{};var f=new FormWindow(Strings.Tr("高级手动配置"));var name=f.Text(Strings.Tr("名称"),p.Name);var arch=f.Text(Strings.Tr("架构"),p.Architecture);var kindKeys=new[]{"linux","bsd","windows","windows-pe-iso","fit","raw","solaris"};var kind=f.Choose(Strings.Tr("类型"),kindKeys.Select(k=>k=="windows-pe-iso"?Strings.Tr("Windows PE 系统盘"):k).ToArray(),Array.IndexOf(kindKeys,p.Kind));var root=f.Text(Strings.Tr("目录"),p.Root);var kernel=f.Text(Strings.Tr("内核 / 引导程序"),p.Kernel);var initrd=f.Text("Initrd",p.Initrd);var dtb=f.Text("DTB",p.Dtb);var args=f.Text(Strings.Tr("参数"),p.Arguments);var mapping=f.Text(Strings.Tr("启动文件映射"),string.Join("\n",p.BootFiles.Select(k=>$"{k.Key}={k.Value}")),true);if(await f.ShowDialog<bool>(this)){p.Name=name.Text??"";p.Architecture=arch.Text??"";p.Kind=kind.SelectedIndex>=0?kindKeys[kind.SelectedIndex]:"raw";p.Root=root.Text??"";p.Kernel=kernel.Text??"";p.Initrd=initrd.Text??"";p.Dtb=dtb.Text??"";p.Arguments=args.Text??"";p.BootFiles=(mapping.Text??"").Split('\n',StringSplitOptions.RemoveEmptyEntries).Select(line=>line.Split('=',2)).ToDictionary(parts=>parts[0].Trim(),parts=>parts.Length==2?parts[1].Trim():throw new Exception(Tr("请按 x64-efi=bootx64.efi 的格式填写，每行一项")));await api.Send("/api/profiles",p);await Refresh();}}
    async Task Network(){
        if(current==null)return;
        var settings=System.Text.Json.Nodes.JsonNode.Parse(current.Settings.GetRawText())!;
        var f=new FormWindow(T("网络设置","Network settings"),T("保存","Save")){Height=Math.Min(660,Math.Max(520,ClientSize.Height-40))};
        var result=new StackPanel{Spacing=8};
        var recheck=new Button{Content=T("重新检查","Recheck")};
        void RenderReport(){
            result.Children.Clear();
            result.Children.Add(new TextBlock{Text="DHCP  ·  "+DhcpStatus(),TextWrapping=Avalonia.Media.TextWrapping.Wrap});
            result.Children.Add(new TextBlock{Text=T("权限与组件","Permissions & components")+"  ·  "+(permissions is null?T("未检查","Not checked"):permissions.Issues.Length>0?T("需要处理","Needs attention"):T("正常","Ready")),TextWrapping=Avalonia.Media.TextWrapping.Wrap});
            if(current?.Running==true)result.Children.Add(new TextBlock{Text=T("停止服务后可重新检测 DHCP","Stop services to recheck DHCP"),TextWrapping=Avalonia.Media.TextWrapping.Wrap});
            var issues=new[]{networkCheckError}.Concat(permissions?.Issues??[]).Where(x=>!string.IsNullOrWhiteSpace(x)).Select(x=>AccessGuidance.Describe(x!)).Distinct();
            foreach(var issue in issues){
                result.Children.Add(new TextBlock{Text=issue,TextWrapping=Avalonia.Media.TextWrapping.Wrap});
            }
            foreach(var url in NetworkIssues().Select(AccessGuidance.DownloadUrl).OfType<string>().Distinct()){
                var link=Action(T("打开官方下载页面","Open official download page"),()=>{System.Diagnostics.Process.Start(new System.Diagnostics.ProcessStartInfo(url){UseShellExecute=true});return Task.CompletedTask;});
                result.Children.Add(link);
            }
            if(!string.IsNullOrWhiteSpace(current?.Network?.Detail))result.Children.Add(new Expander{Header=T("检测详情","Check details"),HorizontalAlignment=HorizontalAlignment.Stretch,Content=new SelectableTextBlock{Text=AccessGuidance.Describe(current.Network.Detail),TextWrapping=Avalonia.Media.TextWrapping.Wrap}});
        }
        RenderReport();f.Add(result);f.Add(recheck);
        f.Section(T("网卡与地址","Interface & addresses"));
        var choices=new[]{new NetworkChoice(null,null)}.Concat((current.Network?.Interfaces??[]).Where(i=>i.Mac.Length>0).SelectMany(i=>i.Addresses.Length==0?new[]{new NetworkChoice(i,null)}:i.Addresses.Select(a=>new NetworkChoice(i,a)))).ToArray();
        var selected=Array.FindIndex(choices,c=>c.Interface?.Id==settings["interface"]?.ToString()&&c.Address?.Ip==settings["server_ip"]?.ToString());
        if(selected<0&&!string.IsNullOrEmpty(settings["interface"]?.ToString())) {
            var saved=new NetworkInterfaceInfo(settings["interface"]!.ToString(),settings["interface"]!.ToString(),settings["server_mac"]!.ToString(),false,false,[],[],"");
            choices=choices.Append(new NetworkChoice(saved,new NetworkAddress(settings["server_ip"]!.ToString(),settings["subnet"]!.ToString()))).ToArray();selected=choices.Length-1;
        }
        var showSelector=current.Network?.ShowInterfaceSelector==true;
        var picker=showSelector?f.Choose(Tr("网卡与 IPv4"),choices,selected):null;
        var address=f.Text(Tr("Server IPv4"),settings["server_ip"]!.ToString());
        var subnet=f.Text(Tr("子网掩码"),settings["subnet"]!.ToString());
        f.Section(T("启动服务","Boot services"));
        var automatic=f.Check(Tr("自动检测 DHCP"),settings["dhcp_auto"]?.GetValue<bool>()??false);
        var dhcp=f.Check(T("分配地址","Allocate addresses"),settings["dhcp_enabled"]!.GetValue<bool>());
        var proxy=f.Check(T("ProxyDHCP（外部 DHCP 分配地址）","ProxyDHCP (external DHCP assigns addresses)"),settings["proxy_dhcp_enabled"]?.GetValue<bool>()??false);
        void Mode(){dhcp.IsEnabled=proxy.IsEnabled=automatic.IsChecked!=true;}
        automatic.IsCheckedChanged+=(_,_)=>Mode();Mode();
        dhcp.IsCheckedChanged+=(_,_)=>{if(dhcp.IsChecked==true)proxy.IsChecked=false;};proxy.IsCheckedChanged+=(_,_)=>{if(proxy.IsChecked==true)dhcp.IsChecked=false;};
        var start=new TextBox{Text=settings["pool_start"]!.ToString()};var end=new TextBox{Text=settings["pool_end"]!.ToString()};
        var pool=new StackPanel{Spacing=10,Children={new TextBlock{Text=Tr("地址池起点")},start,new TextBlock{Text=Tr("地址池终点")},end}};f.Add(pool);
        void PoolMode(){pool.IsVisible=dhcp.IsChecked==true && automatic.IsChecked!=true;}
        dhcp.IsCheckedChanged+=(_,_)=>PoolMode();automatic.IsCheckedChanged+=(_,_)=>PoolMode();PoolMode();
        f.BeginAdvanced(T("高级设置","Advanced settings"));
        var bind=f.Text(Tr("Listen IPv4"),settings["bind_ip"]!.ToString());
        var iface=showSelector?f.Text(Tr("网卡"),settings["interface"]!.ToString()):new TextBox{Text=settings["interface"]!.ToString()};
        var mac=f.Text(Tr("Adapter MAC"),settings["server_mac"]!.ToString());
        var ethernet=f.Check(Tr("以太网协议"),settings["ethernet_enabled"]!.GetValue<bool>());
        if(picker is not null)picker.SelectionChanged+=(_,_)=>{if(picker.SelectedItem is NetworkChoice c){iface.Text=c.Interface?.Id??"";if(c.Interface is not null){mac.Text=c.Interface.Mac;ethernet.IsChecked=true;}if(c.Address is not null){address.Text=bind.Text=c.Address.Ip;subnet.Text=c.Address.Subnet;}}};
        recheck.Click+=async(_,_)=>{
            f.SetBusy(true);recheck.IsEnabled=false;recheck.Content=Tr("检测中…");
            try{await CheckNetwork(true);RenderReport();}
            finally{f.SetBusy(false);recheck.IsEnabled=true;recheck.Content=T("重新检查","Recheck");}
        };
        if(await f.ShowDialog<bool>(this)){
            settings["server_ip"]=address.Text;settings["bind_ip"]=bind.Text;settings["pool_start"]=start.Text;settings["pool_end"]=end.Text;settings["subnet"]=subnet.Text;
            settings["dhcp_auto"]=automatic.IsChecked==true;settings["dhcp_enabled"]=automatic.IsChecked!=true&&dhcp.IsChecked==true;settings["proxy_dhcp_enabled"]=automatic.IsChecked!=true&&proxy.IsChecked==true;
            settings["interface"]=iface.Text;settings["server_mac"]=mac.Text;settings["ethernet_enabled"]=ethernet.IsChecked==true;
            await api.Send("/api/settings",settings,HttpMethod.Put);await CheckNetwork(false);await Refresh();
        }
    }
    async Task RemoveDevice(){if(devices.SelectedItem is Device d){await api.Send($"/api/devices/{d.Id}",method:HttpMethod.Delete);devices.SelectedItem=null;await Refresh();}}
    async Task RemoveProfile(){if(systems.SelectedItem is Profile p){await api.Send($"/api/profiles/{p.Id}",method:HttpMethod.Delete);systems.SelectedItem=null;await Refresh();}}
    async Task Uboot(){if(systems.SelectedItem is not Profile p)return;var f=new FormWindow("U-Boot");var box=f.Text(Strings.Tr("命令"),await api.Uboot(p.Id),true);box.IsReadOnly=true;await f.ShowDialog<bool>(this);}
    async Task Commands(){if(systems.SelectedItem is not Profile p)return;var result=await api.BootCommands(p.Id,Chinese?"zh":"en");var status=result.Commands.Length>0?Tr("在固件中执行以下命令"):result.Firmware is "pxe" or "efi"?Tr("从固件菜单选择网络启动，无需输入命令"):result.Firmware=="openfirmware"?Tr("请使用固件的网络启动入口"):Tr("请先完成启动配置，详见下方说明");await new CommandWindow(p.Name,result.Title,status,result.Commands,result.Notes,T("复制命令","Copy commands"),T("已复制","Copied"),T("关闭","Close")).ShowDialog<bool>(this);}
    async Task About(){
        using var stream=typeof(MainWindow).Assembly.GetManifestResourceStream("Bootah.ThirdParty.json")!;
        var entries=System.Text.Json.JsonSerializer.Deserialize<ThirdPartyComponent[]>(stream,new System.Text.Json.JsonSerializerOptions{PropertyNameCaseInsensitive=true})!;
        var platform=OperatingSystem.IsWindows()?"windows":"linux";
        var window=new Window{Title=Tr("关于与开源许可"),Width=740,Height=620,WindowStartupLocation=WindowStartupLocation.CenterOwner};
        var root=new DockPanel{Margin=new Thickness(24)};
        var title=new TextBlock{Text="Bootah",FontSize=26,FontWeight=Avalonia.Media.FontWeight.SemiBold,Margin=new Thickness(0,0,0,18)};DockPanel.SetDock(title,Dock.Top);root.Children.Add(title);
        CoreVersion? coreVersion=null;try{coreVersion=(await api.State()).CoreVersion;}catch{}
        var versions=new StackPanel{Spacing=4,Margin=new Thickness(0,0,0,12)};
        versions.Children.Add(new TextBlock{Text=$"{Tr("客户端版本")}: {ClientVersion.Display} ({ClientVersion.Build})"});
        versions.Children.Add(new TextBlock{Text=$"{Tr("核心版本")}: {(coreVersion is null?Tr("不可用"):$"{coreVersion.Display} ({coreVersion.Build})")}"});
        DockPanel.SetDock(versions,Dock.Top);root.Children.Add(versions);
        var close=new Button{Content=Tr("关闭"),HorizontalAlignment=HorizontalAlignment.Right,Margin=new Thickness(0,16,0,0)};close.Click+=(_,_)=>window.Close();DockPanel.SetDock(close,Dock.Bottom);root.Children.Add(close);
        Button Link(string text,Func<Task> action){var button=Action(text,action);button.Content=new TextBlock{Text=text,TextWrapping=Avalonia.Media.TextWrapping.Wrap,TextDecorations=Avalonia.Media.TextDecorations.Underline};button.Padding=new Thickness(0);button.Background=Avalonia.Media.Brushes.Transparent;button.BorderThickness=new Thickness(0);button.HorizontalAlignment=HorizontalAlignment.Left;return button;}
        Control LabeledLink(string label,string value,Func<Task> action){var row=new Grid{ColumnDefinitions=new ColumnDefinitions("Auto,*")};row.Children.Add(new TextBlock{Text=Tr(label)+":",Margin=new Thickness(0,0,6,0),VerticalAlignment=VerticalAlignment.Top});var link=Link(value,action);Grid.SetColumn(link,1);row.Children.Add(link);return row;}
        Task OpenSource(string url){System.Diagnostics.Process.Start(new System.Diagnostics.ProcessStartInfo(url){UseShellExecute=true});return Task.CompletedTask;}
        var list=new ListBox{ItemsSource=entries.Where(e=>e.Platforms.Contains(platform)).ToArray(),Background=Avalonia.Media.Brushes.Transparent,BorderThickness=new Thickness(0)};
        list.Styles.Add(new Avalonia.Styling.Style(x=>x.OfType<ListBoxItem>()){Setters={new Avalonia.Styling.Setter(ListBoxItem.HorizontalContentAlignmentProperty,HorizontalAlignment.Stretch)}});
        list.ItemTemplate=new Avalonia.Controls.Templates.FuncDataTemplate<ThirdPartyComponent>((entry,_)=>{
            if(entry is null)return new TextBlock();
            return new StackPanel{Spacing=5,Margin=new Thickness(0,10),Children={
                new TextBlock{Text=entry.Name+(entry.Version.Length>0?"  "+entry.Version:""),FontSize=16,FontWeight=Avalonia.Media.FontWeight.SemiBold},
                LabeledLink("来源",entry.Source,()=>OpenSource(entry.Source)),
                LabeledLink("许可协议",Tr(entry.License),async()=>{if(entry.Notice.Length==0){await OpenSource(entry.LicenseUrl);return;}var viewer=new Window{Title=entry.Name+" — "+Tr("许可协议"),Width=720,Height=560,WindowStartupLocation=WindowStartupLocation.CenterOwner};var panel=new DockPanel{Margin=new Thickness(20)};var done=new Button{Content=Tr("关闭"),HorizontalAlignment=HorizontalAlignment.Right,Margin=new Thickness(0,12,0,0)};done.Click+=(_,_)=>viewer.Close();DockPanel.SetDock(done,Dock.Bottom);panel.Children.Add(done);panel.Children.Add(new TextBox{Text=entry.Notice,IsReadOnly=true,AcceptsReturn=true,TextWrapping=Avalonia.Media.TextWrapping.Wrap});viewer.Content=panel;await viewer.ShowDialog(window);})
            }};
        });root.Children.Add(list);window.Content=root;await window.ShowDialog(this);
    }

}
class FormWindow:Window {
    readonly StackPanel fields=new(){Spacing=10,Margin=new Thickness(22)};
    StackPanel? advanced;
    Control? buttons;
    StackPanel Fields=>advanced??fields;
    public FormWindow(string title,string? confirmLabel=null){Title=title;Width=520;Height=600;WindowStartupLocation=WindowStartupLocation.CenterOwner;var root=new DockPanel();var bar=new StackPanel{Orientation=Orientation.Horizontal,HorizontalAlignment=HorizontalAlignment.Right,Spacing=8,Margin=new Thickness(20)};var cancel=new Button{Content=Strings.Tr("取消")};cancel.Click+=(_,_)=>Close(false);var save=new Button{Content=confirmLabel??Strings.Tr("确定")};save.Click+=(_,_)=>Close(true);bar.Children.Add(cancel);bar.Children.Add(save);buttons=bar;DockPanel.SetDock(bar,Dock.Bottom);root.Children.Add(bar);root.Children.Add(new ScrollViewer{Content=fields});Content=root;}
    public TextBox Text(string label,string value,bool multiline=false){Fields.Children.Add(new TextBlock{Text=label});var box=new TextBox{Text=value,AcceptsReturn=multiline,MinHeight=multiline?110:32};Fields.Children.Add(box);return box;}
    public void Add(Control control)=>Fields.Children.Add(control);
    public void Section(string title)=>Fields.Children.Add(new TextBlock{Text=title,FontWeight=Avalonia.Media.FontWeight.SemiBold,Margin=new Thickness(0,14,0,0)});
    public void Note(string value){Fields.Children.Add(new TextBlock{Text=value,Opacity=.65,TextWrapping=Avalonia.Media.TextWrapping.Wrap});}
    public ComboBox Choose<T>(string label,T[] values,int selected){Fields.Children.Add(new TextBlock{Text=label});var box=new ComboBox{ItemsSource=values,SelectedIndex=Math.Max(0,selected),HorizontalAlignment=HorizontalAlignment.Stretch};Fields.Children.Add(box);return box;}
    public CheckBox Check(string label,bool value){var box=new CheckBox{Content=label,IsChecked=value};Fields.Children.Add(box);return box;}
    public void SetBusy(bool busy){if(buttons is not null)buttons.IsEnabled=!busy;}
    public void BeginAdvanced(string title){advanced=new StackPanel{Spacing=10};fields.Children.Add(new Expander{Header=title,Content=advanced,HorizontalAlignment=HorizontalAlignment.Stretch});}

}
class CommandWindow:Window {
    public CommandWindow(string name,string title,string status,string commands,string[] notes,string copyLabel,string copiedLabel,string closeLabel) {
        Title=title;Width=700;Height=540;WindowStartupLocation=WindowStartupLocation.CenterOwner;
        var root=new DockPanel{Margin=new Thickness(22)};
        var header=new StackPanel{Spacing=6};
        header.Children.Add(new TextBlock{Text=name,Opacity=.65});
        header.Children.Add(new TextBlock{Text=title,FontSize=22,FontWeight=Avalonia.Media.FontWeight.SemiBold});
        header.Children.Add(new TextBlock{Text=status,FontSize=12,Opacity=.8,TextWrapping=Avalonia.Media.TextWrapping.Wrap});
        DockPanel.SetDock(header,Dock.Top);root.Children.Add(header);
        var bar=new StackPanel{Orientation=Orientation.Horizontal,HorizontalAlignment=HorizontalAlignment.Right,Spacing=8,Margin=new Thickness(0,16,0,0)};
        var close=new Button{Content=closeLabel};close.Click+=(_,_)=>Close(true);
        bar.Children.Add(close);DockPanel.SetDock(bar,Dock.Bottom);root.Children.Add(bar);
        var content=new StackPanel{Spacing=10};
        if(commands.Length>0){
            content.Children.Add(new TextBox{Text=commands,IsReadOnly=true,AcceptsReturn=true,FontFamily=new Avalonia.Media.FontFamily("monospace"),MinHeight=110,MaxHeight=240,TextWrapping=Avalonia.Media.TextWrapping.NoWrap});
            var copy=new Button{Content=copyLabel,HorizontalAlignment=HorizontalAlignment.Right};
            copy.Click+=async (_,_)=>{var clipboard=TopLevel.GetTopLevel(this)?.Clipboard;if(clipboard is not null)await clipboard.SetTextAsync(commands);copy.Content=copiedLabel;};
            content.Children.Add(copy);
        }
        if(notes.Length>0){
            var list=new StackPanel{Spacing=6};
            foreach(var note in notes)list.Children.Add(new TextBlock{Text="• "+Strings.Message(note),TextWrapping=Avalonia.Media.TextWrapping.Wrap,Opacity=.75});
            content.Children.Add(new ScrollViewer{Content=list,MaxHeight=190});
        }
        root.Children.Add(new ScrollViewer{Content=content});
        Content=root;
    }
}

record ThirdPartyComponent(string Name,string Version,string Source,string License,string Notice,string LicenseUrl,string[] Platforms);
