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
    readonly TextBlock status=new() {Text=Strings.Tr("Disconnected"),VerticalAlignment=VerticalAlignment.Center,MaxWidth=150,TextWrapping=Avalonia.Media.TextWrapping.Wrap};
    readonly Button service=new(){Content=Strings.Tr("Start")};
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
    static string Tr(string key)=>Strings.Tr(key);
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
            p?.Status=="needs-attention"?Tr("Needs setup"):"",LogoId(p)));
        devices.DoubleTapped+=async(_,_)=>await Run(()=>EditDevice(devices.SelectedItem as Device));
        devices.SelectionChanged+=(_,_)=>UpdateAutomationButtons();systems.SelectionChanged+=(_,_)=>UpdateAutomationButtons();
        systems.DoubleTapped+=async(_,_)=>await Run(()=>EditProfile(systems.SelectedItem as Profile));
        var rail=new StackPanel{Spacing=8,Margin=new Thickness(16,24)};
        rail.Children.Add(new TextBlock{Text="Bootah",FontSize=23,FontWeight=Avalonia.Media.FontWeight.SemiBold,Margin=new Thickness(12,0,0,32)});
        foreach(var name in new[]{"Devices","Image library","Activity"}){var button=Action(PageName(name),()=>{ShowPage(name);return Task.CompletedTask;});button.HorizontalAlignment=HorizontalAlignment.Stretch;button.HorizontalContentAlignment=HorizontalAlignment.Left;button.Padding=new Thickness(16,12);button.Background=Avalonia.Media.Brushes.Transparent;button.BorderThickness=new Thickness(0);navigation[name]=button;rail.Children.Add(button);}
        var side=new DockPanel{LastChildFill=false};DockPanel.SetDock(rail,Dock.Top);side.Children.Add(rail);
        var connection=new StackPanel{Spacing=12,Margin=new Thickness(28),Children={status,networkStatus,Action(Tr("Network settings"),Network)}};DockPanel.SetDock(connection,Dock.Bottom);side.Children.Add(connection);
        if(Environment.GetEnvironmentVariable("NETBOOT_API") is not null)connection.Children.Add(Action(Tr("Management token"),EditToken));
        var main=new DockPanel{Margin=new Thickness(32,26)};
        var header=new DockPanel{Margin=new Thickness(0,0,0,24)};
        service.Classes.Add("accent");service.Padding=new Thickness(22,10);DockPanel.SetDock(service,Dock.Right);header.Children.Add(service);var about=Action(Tr("About & open-source licenses"),About);DockPanel.SetDock(about,Dock.Right);header.Children.Add(about);header.Children.Add(pageTitle);DockPanel.SetDock(header,Dock.Top);main.Children.Add(header);
        tools.Margin=new Thickness(0,0,0,18);DockPanel.SetDock(tools,Dock.Top);main.Children.Add(tools);main.Children.Add(body);
        var grid=new Grid{ColumnDefinitions=new ColumnDefinitions("220,*")};grid.Children.Add(new Border{Background=Tone("#EEF2F7","#22262E"),Child=side});Grid.SetColumn(main,1);grid.Children.Add(main);Content=grid;
        logLevel.ItemsSource=new[]{Tr("Errors"),Tr("Standard"),Tr("Detailed")};
        includePrivate.Content=Tr("Include private data");
        ShowPage("Devices");
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
        var options=new[]{new Profile{Name=automaticName is null?Tr("Unassigned"):$"{Tr("Assign by architecture")}  {automaticName}"}}.Concat(current?.Profiles??Array.Empty<Profile>()).ToArray();
        var selector=new ComboBox{ItemsSource=options,SelectedIndex=Math.Max(0,Array.FindIndex(options,p=>p.Id==device.ProfileId)),MinWidth=190,MaxWidth=260,HorizontalAlignment=HorizontalAlignment.Right};
        selector.SelectionChanged+=async (_,_)=>{if(selector.SelectedItem is not Profile selected||selected.Id==device.ProfileId)return;await Run(async()=>{var changed=device with{ProfileId=string.IsNullOrEmpty(selected.Id)?null:selected.Id};await api.Send("/api/devices",changed);await Refresh();});};
        var row=new Grid{ColumnDefinitions=new ColumnDefinitions("*,Auto"),Margin=new Thickness(20,14)};
        row.Children.Add(new StackPanel{Spacing=5,Children={new TextBlock{Text=string.IsNullOrWhiteSpace(device.Name)?(string.IsNullOrWhiteSpace(device.Mac)?device.Ip??Tr("IP binding"):device.Mac):device.Name,FontSize=16,FontWeight=Avalonia.Media.FontWeight.SemiBold},new TextBlock{Text=string.Join("  ",new[]{device.Ip,device.Architecture,device.Model}.Where(x=>!string.IsNullOrWhiteSpace(x))),FontSize=12,Opacity=.65}}});
        Grid.SetColumn(selector,1);row.Children.Add(selector);
        return new Border{Child=row,CornerRadius=new CornerRadius(10),Background=Tone("#FFFFFF","#282D36"),Margin=new Thickness(0,0,0,8)};
    }
    static string PageName(string name)=>name switch{"Devices"=>Tr("Devices & profiles"),"Image library"=>Tr("Media & import"),_=>Tr("Transfers & logs")};
    void ShowPage(string name){foreach(var pair in navigation){pair.Value.Background=pair.Key==name?Tone("#DCE8F8","#33445D"):Avalonia.Media.Brushes.Transparent;}pageTitle.Text=PageName(name);tools.Children.Clear();automationButtons.Clear();deleteButtons.Clear();if(name=="Devices"){body.Content=devices;tools.Children.Add(Action(Tr("Add device"),()=>EditDevice(null)));tools.Children.Add(Action(Tr("Assign image"),()=>EditDevice(devices.SelectedItem as Device)));tools.Children.Add(AutomationButton("device"));tools.Children.Add(DeleteButton("device"));}else if(name=="Image library"){var mediaLayout=new DockPanel();var defaultsScroll=new ScrollViewer{Content=architectureDefaults,MaxHeight=240,Margin=new Thickness(0,0,0,16)};DockPanel.SetDock(defaultsScroll,Dock.Top);mediaLayout.Children.Add(defaultsScroll);mediaLayout.Children.Add(systems);body.Content=mediaLayout;RenderArchitectureDefaults();var import=Action(Tr("Import ISO"),ImportIso);import.Classes.Add("accent");tools.Children.Add(import);trustLocalIso.Content=Tr("Trust selected local ISO source");tools.Children.Add(trustLocalIso);tools.Children.Add(Action(Tr("Add manually"),()=>EditProfile(null)));tools.Children.Add(Action(Tr("Image settings"),()=>EditProfile(systems.SelectedItem as Profile)));tools.Children.Add(AutomationButton("profile"));tools.Children.Add(Action(Tr("Boot options"),Commands));tools.Children.Add(DeleteButton("profile"));}else{body.Content=events;tools.Children.Add(logLevel);tools.Children.Add(includePrivate);tools.Children.Add(Action(Tr("Clear logs"),async()=>{await api.Send("/api/events",new{},HttpMethod.Delete);await Refresh();}));tools.Children.Add(Action(Tr("Back to top"),()=>{if(events.ItemCount>0 && events.Items[0] is object first)events.ScrollIntoView(first);return Task.CompletedTask;}));tools.Children.Add(Action(Tr("Export diagnostics"),ExportDiagnostics));}foreach(var control in tools.Children){control.Margin=new Thickness(0,0,name=="Activity"?20:8,8);control.VerticalAlignment=Avalonia.Layout.VerticalAlignment.Center;}UpdateAutomationButtons();}
    async Task ExportDiagnostics(){
        if(!File.Exists(App.CoreBinary))throw new Exception(Tr("Local core executable not found"));
        if(includePrivate.IsChecked==true){var confirm=new FormWindow(Tr("Raw logs contain private data"));confirm.Note(Tr("Raw logs may contain device addresses and local file paths."));if(!await confirm.ShowDialog<bool>(this))return;}
        var picked=await StorageProvider.SaveFilePickerAsync(new Avalonia.Platform.Storage.FilePickerSaveOptions{Title=Tr("Export diagnostics"),SuggestedFileName="bootah-diagnostics.txt"});if(picked is null)return;
        var start=new System.Diagnostics.ProcessStartInfo(App.CoreBinary){UseShellExecute=false,CreateNoWindow=true,RedirectStandardOutput=true,RedirectStandardError=true};
        foreach(var value in new[]{"--export-diagnostics","--config",App.CoreConfig,"--log-level",new[]{"error","info","debug"}[Math.Clamp(logLevel.SelectedIndex,0,2)]})start.ArgumentList.Add(value);
        if(includePrivate.IsChecked==true)start.ArgumentList.Add("--include-private");
        using var process=System.Diagnostics.Process.Start(start)??throw new Exception(Tr("Could not start diagnostics export"));
        var output=process.StandardOutput.ReadToEndAsync();var error=process.StandardError.ReadToEndAsync();await process.WaitForExitAsync();
        if(process.ExitCode!=0)throw new Exception(await error);
        await using var stream=await picked.OpenWriteAsync();var bytes=System.Text.Encoding.UTF8.GetBytes(await output);await stream.WriteAsync(bytes);await stream.FlushAsync();
    }
    Profile? AutomationProfile(string scope,string? id)=>scope=="profile"?current?.Profiles.FirstOrDefault(p=>p.Id==id):current?.Devices.FirstOrDefault(d=>d.Id==id) is Device d?current?.Profiles.FirstOrDefault(p=>p.Id==(d.ProfileId??current.DefaultProfiles.GetValueOrDefault(d.Architecture))):null;
    Control AutomationButton(string scope){
        var button=Action(Tr("Automatic installation"),()=>ConfigureAutomation(scope,scope=="profile"?(systems.SelectedItem as Profile)?.Id:(devices.SelectedItem as Device)?.Id));
        var host=new Border{Child=button,Background=Avalonia.Media.Brushes.Transparent};automationButtons.Add((host,button,scope));return host;
    }
    Button DeleteButton(string scope){
        var button=Action(Tr("Delete"),scope=="device"?RemoveDevice:RemoveProfile);
        deleteButtons.Add((button,scope));return button;
    }
    void UpdateAutomationButtons(){
        foreach(var (button,scope) in deleteButtons)button.IsEnabled=scope=="device"?devices.SelectedItem is Device:systems.SelectedItem is Profile;
        foreach(var (host,button,scope) in automationButtons){
            var id=scope=="profile"?(systems.SelectedItem as Profile)?.Id:(devices.SelectedItem as Device)?.Id;
            var p=AutomationProfile(scope,id);
            var loaded=p is not null&&automationCapabilities.TryGetValue(p.Id,out var capability)&&capability.Signature==System.Text.Json.JsonSerializer.Serialize(p,Api.Json);
            button.IsEnabled=loaded&&automationCapabilities[p!.Id].Methods.Length>0;
            ToolTip.SetTip(host,button.IsEnabled?null:Tr(id is null?"Select a device or image first":p is null?"Assign a boot image first":loaded?"This platform does not support automatic configuration":"Loading automatic configuration options…"));
        }
    }
    async Task LoadAutomationCapabilities(){
        if(current is null)return;
        foreach(var p in current.Profiles){var signature=System.Text.Json.JsonSerializer.Serialize(p,Api.Json);if(automationCapabilities.TryGetValue(p.Id,out var cached)&&cached.Signature==signature)continue;automationCapabilities[p.Id]=(signature,await api.AutomationMethods(p.Id));}
        UpdateAutomationButtons();
    }
    async Task ConfigureAutomation(string scope,string? id){if(current is null||string.IsNullOrEmpty(id))return;var profile=scope=="profile"?current.Profiles.FirstOrDefault(p=>p.Id==id):current.Devices.FirstOrDefault(d=>d.Id==id) is Device d?current.Profiles.FirstOrDefault(p=>p.Id==(d.ProfileId??current.DefaultProfiles.GetValueOrDefault(d.Architecture))):null;if(profile is null)return;var methods=await api.AutomationMethods(profile.Id);if(methods.Length==0)return;var existing=current.Automations?.GetValueOrDefault($"{scope}:{id}");var choices=new[]{Tr("No automatic configuration")}.Concat(methods).ToArray();var form=new FormWindow(Tr("Automatic installation"));form.Note($"{profile.Name}  {existing?.Filename??Tr("Not configured")}");var selected=form.Choose(Tr("Configuration method"),choices,Math.Max(0,Array.IndexOf(choices,existing?.Method)));if(!await form.ShowDialog<bool>(this))return;var index=selected.SelectedIndex;if(index<=0){await api.ClearAutomation(scope,id);await Refresh();return;}var picked=await StorageProvider.OpenFilePickerAsync(new Avalonia.Platform.Storage.FilePickerOpenOptions{Title=Tr("Select configuration file"),AllowMultiple=false});if(picked.Count==0)return;await api.SetAutomation(scope,id,methods[index-1],picked[0].Path.LocalPath);await Refresh();}
    void RenderArchitectureDefaults(){
        architectureDefaults.Children.Clear();
        architectureDefaults.Children.Add(new TextBlock{Text=Tr("Default boot images by architecture"),FontSize=16});
        if(current is null)return;
        var rows=new List<Control>();
        foreach(var key in current.Profiles.SelectMany(p=>p.BootFiles.Keys).Where(FirmwareKeys.Contains).Distinct().OrderBy(x=>x)){
            var name=current.Profiles.FirstOrDefault(p=>p.Id==current.DefaultProfiles.GetValueOrDefault(key))?.Name??Tr("Not configured");
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
        var options=new[]{new Profile{Name=Tr("Not configured")}}.Concat(current.Profiles.Where(p=>p.BootFiles.ContainsKey(key))).ToArray();
        var form=new FormWindow(key);
        var choice=form.Choose(Tr("Boot image"),options.Select(p=>p.Name).ToArray(),Math.Max(0,Array.FindIndex(options,p=>p.Id==current.DefaultProfiles.GetValueOrDefault(key))));
        choice.MaxWidth=440;choice.ItemTemplate=new Avalonia.Controls.Templates.FuncDataTemplate<string>((name,_)=>{
            var label=new TextBlock{Text=name,TextTrimming=Avalonia.Media.TextTrimming.CharacterEllipsis,TextWrapping=Avalonia.Media.TextWrapping.NoWrap,MaxWidth=420};
            ToolTip.SetTip(label,name);return label;
        });
        if(!await form.ShowDialog<bool>(this)||current is null)return;
        var defaults=new Dictionary<string,string>(current.DefaultProfiles);var id=options[choice.SelectedIndex].Id;
        if(string.IsNullOrEmpty(id))defaults.Remove(key);else defaults[key]=id;
        await api.Send("/api/default-profiles",defaults,HttpMethod.Put);await Refresh();
    }
    static readonly HashSet<string> FirmwareKeys=["x86-bios","ia64","ia32-efi","x64-efi","ebc-efi","alpha","sparc64","powerpc-of","powerpc-epapr","power-opal","arm32-efi","arm64-efi","ia32-http","x64-http","arm32-http","arm64-http","arm32-uboot","arm64-uboot","riscv32-efi","riscv64-http","loongarch64-efi","riscv64-efi","hp-rmp"];
    async Task ImportIso(){
        var trusted=trustLocalIso.IsChecked==true;trustLocalIso.IsChecked=false;
        var picked=await StorageProvider.OpenFilePickerAsync(new Avalonia.Platform.Storage.FilePickerOpenOptions{Title=Tr("Import ISO"),AllowMultiple=false,FileTypeFilter=new[]{new Avalonia.Platform.Storage.FilePickerFileType("ISO"){Patterns=new[]{"*.iso"}}}});
        if(picked.Count==0)return;status.Text=Tr("Importing…");tools.IsEnabled=false;
        try{var result=await api.Import(picked[0].Path.LocalPath,trusted);await Refresh();status.Text=Tr("ISO imported");var issues=result.Warnings;if(issues.Length>0){var form=new FormWindow(Tr("Action required"));foreach(var issue in issues)form.Note(Strings.Message(issue));await form.ShowDialog<bool>(this);}}
        finally{tools.IsEnabled=true;}
    }

    static bool ActionableDiagnostic(BootDiagnostic d)=>d.Code is not ("BOOT_CONFIG_FAMILY_LAYOUT" or "UOS_MANUAL_ONLY");
    static string SourceLabel(string source)=>source switch{"http-tree"=>Tr("HTTP installation tree"),"http-iso"=>Tr("HTTP ISO"),"nfs-tree"=>Tr("Read-only NFS file tree"),"nbd"=>Tr("Read-only NBD ISO"),"external"=>Tr("External installation source"),"local-media"=>Tr("Local media required"),_=>Tr("Unrecognized installation source")};
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
        "lease_found"=>Tr("DHCP record found"),
        "response_found"=>Tr("DHCP server found"),
        "probe_unavailable"=>Tr("Check unavailable"),
        null=>Tr("Not checked"),
        var value=>Tr(value)
    };
    string[] NetworkIssues()=>new[]{networkCheckError,current?.Network?.Detail}
        .Concat(permissions?.Issues??[]).Where(x=>!string.IsNullOrWhiteSpace(x)).Select(x=>AccessGuidance.Describe(x!)).Distinct().ToArray();
    void UpdateNetworkStatus(){
        networkStatus.Text=checkingNetwork?Tr("Detecting…"):networkCheckError is not null?Tr("Check failed"):
            NetworkIssues().Length>0?Tr("Network needs attention"):DhcpStatus();
        ToolTip.SetTip(networkStatus,null);
    }
    internal async Task ShowFailure(string message){
        message=Strings.Tr(message);
        var window=new Window{Title="Bootah",Width=620,Height=330,WindowStartupLocation=WindowStartupLocation.CenterOwner};
        var panel=new DockPanel{Margin=new Thickness(20)};
        var close=new Button{Content=Tr("OK"),HorizontalAlignment=HorizontalAlignment.Right,Margin=new Thickness(0,16,0,0)};
        close.Click+=(_,_)=>window.Close();DockPanel.SetDock(close,Dock.Bottom);panel.Children.Add(close);
        panel.Children.Add(new ScrollViewer{Content=new SelectableTextBlock{Text=AccessGuidance.Describe(message),TextWrapping=Avalonia.Media.TextWrapping.Wrap}});
        if(AccessGuidance.DownloadUrl(message) is string url){
            var download=new Button{Content=Tr("Open official download page"),Margin=new Thickness(0,12,0,0)};
            download.Click+=(_,_)=>{try{System.Diagnostics.Process.Start(new System.Diagnostics.ProcessStartInfo(url){UseShellExecute=true});}catch(Exception e){download.Content=e.Message;}};
            DockPanel.SetDock(download,Dock.Bottom);panel.Children.Add(download);
        }
        window.Content=panel;await window.ShowDialog(this);
    }
    async Task Run(Func<Task> action){try{await action();}catch(Exception e){await ShowFailure(e.Message);}}
    async Task Refresh(){if(refreshing||checkingNetwork)return;refreshing=true;try{current=await api.State();RenderArchitectureDefaults();UpdateNetworkStatus();var id=(devices.SelectedItem as Device)?.Id;var pid=(systems.SelectedItem as Profile)?.Id;devices.ItemsSource=current.Devices;systems.ItemsSource=current.Profiles;devices.SelectedItem=current.Devices.FirstOrDefault(d=>d.Id==id);systems.SelectedItem=current.Profiles.FirstOrDefault(p=>p.Id==pid);events.ItemsSource=current.Events.Reverse().Take(100).Select(e=>$"{DateTimeOffset.FromUnixTimeSeconds(e.GetProperty("time").GetInt64()).ToLocalTime():HH:mm:ss}  {e.GetProperty("device").GetString()}  {e.GetProperty("protocol").GetString()}  {e.GetProperty("message").GetString()}").ToArray();status.Text=Tr(current.Running?"Running":"Stopped");service.Content=Tr(current.Running?"Stop":"Start");await LoadAutomationCapabilities();if(!networkChecked){networkChecked=true;await CheckNetwork(true);if(NetworkIssues().Length>0)await Network();}if(current.DeviceConflicts?.FirstOrDefault() is DeviceConflict conflict)await ConfirmDeviceConflict(conflict);}catch(Exception e){status.Text=$"{Tr("Disconnected")}  {Strings.Tr(e.Message)}";}finally{refreshing=false;}}
    async Task ConfirmDeviceConflict(DeviceConflict conflict){
        var details=conflict.Reason=="ip-mac"?$"IP {conflict.Ip}: {conflict.OldMac} → {conflict.NewMac}":$"{conflict.NewMac}: {conflict.OldArchitecture} → {conflict.NewArchitecture}";
        var form=new FormWindow(Tr("Remove the old device record?"));form.Note(details);form.Note(Tr("The old device has manual settings. After removal, the new device will use its default image on the next network boot."));
        var accept=await form.ShowDialog<bool>(this);
        await api.Send($"/api/device-conflicts/{Uri.EscapeDataString(conflict.Id)}",new{accept});
    }
    async Task EditToken(){var form=new FormWindow(Tr("Management token"));var input=form.Text(Tr("Management token"),App.ApiToken??"");input.PasswordChar='●';var save=form.Check(Tr("Save token securely"),true);if(await form.ShowDialog<bool>(this)){App.ApiToken=input.Text?.Trim();var endpoint=Environment.GetEnvironmentVariable("NETBOOT_API")!;if(save.IsChecked==true&&!string.IsNullOrEmpty(App.ApiToken)){if(!TokenStore.Save(endpoint,App.ApiToken)){TokenStore.Remove(endpoint);await ShowFailure(Tr("Could not save the token securely; it will be used for this session only"));}}else TokenStore.Remove(endpoint);await Refresh();}}
    async Task EditDevice(Device? source){if(current==null)return;var d=source is null?new Device():source with{};var form=new FormWindow(Tr("Devices"));form.Note(Tr("Enter a MAC or IP address"));var mac=form.Text("MAC",d.Mac);var ip=form.Text("IP",d.Ip??"");var name=form.Text(Tr("Name"),d.Name);var model=form.Text(Tr("Model"),d.Model);var options=new[]{new Profile{Name=Tr("Unassigned")}}.Concat(current.Profiles).ToArray();var profiles=form.Choose(Tr("Boot image"),options,Array.FindIndex(options,p=>p.Id==d.ProfileId));if(await form.ShowDialog<bool>(this)){if(string.IsNullOrWhiteSpace(mac.Text)&&string.IsNullOrWhiteSpace(ip.Text))throw new Exception(Tr("Enter a MAC or IP address"));d.Name=name.Text??"";d.Mac=mac.Text?.Trim()??"";d.Ip=string.IsNullOrWhiteSpace(ip.Text)?null:ip.Text.Trim();d.Model=model.Text??"";d.ProfileId=(profiles.SelectedItem as Profile)?.Id;if(d.ProfileId=="")d.ProfileId=null;await api.Send("/api/devices",d);await Refresh();}}
    async Task EditProfile(Profile? source){
        var p=source is null?new Profile{Id=Guid.NewGuid().ToString("N")}:source with{};
        if(source is not null && p.SecureBoot?.Eligible==true)p.SecureBoot=await api.SecureBoot(p.Id);
        var secured=p.SecureBoot?.Enabled==true;
        var f=new FormWindow(Tr("Image settings"),Tr("Save"));
        f.Section(Tr("General"));
        var name=f.Text(Tr("Name"),p.Name);
        if(!string.IsNullOrEmpty(p.Media))f.Text(Tr("Image file"),p.Media).IsReadOnly=true;
        var parameterInfo=source is null?null:await api.Parameters(p.Id);
        var parameterGroup=f.BeginGroup();parameterGroup.IsVisible=parameterInfo?.Editable??true;
        f.Section(Tr("Boot parameters"));
        var args=f.Text(p.Kind=="linux"?Tr("Kernel command line"):Tr("Boot parameters"),p.Arguments,true);args.TextWrapping=Avalonia.Media.TextWrapping.Wrap;
        if(p.Kind=="windows")f.Note(Tr("Windows options: bootlog=on sos=on; use entry={GUID} for multiple boot entries."));
        if(source is not null){
            var info=parameterInfo!;
            var reset=Action(Tr("Restore imported parameters"),()=>{args.Text=info.OriginalArguments;return Task.CompletedTask;});reset.IsEnabled=info.OriginalArguments is not null&&info.Editable;f.Add(reset);
            var preview=new TextBox{IsReadOnly=true,AcceptsReturn=true,MinHeight=100,TextWrapping=Avalonia.Media.TextWrapping.Wrap};
            var expanded=new Expander{Header=Tr("Parameter preview"),Content=preview,HorizontalAlignment=HorizontalAlignment.Stretch};
            f.Add(Action(Tr("Preview parameters"),async()=>{var result=await api.Parameters(p.Id,p with{Arguments=args.Text??""});preview.Text=result.Preview;expanded.IsExpanded=true;}));f.Add(expanded);
            if(info.OriginalArguments is null)f.Note(Tr("Imported parameter defaults are unavailable for this image."));
        }
        f.EndAdvanced();
        CheckBox? secureToggle=null;ComboBox? secureMode=null;
        var modes=p.SecureBoot?.Modes??[];
        if(p.SecureBoot?.Eligible==true){
            f.Section(Tr("Boot settings"));
            secureToggle=f.Check(Tr("Secure Boot support"),secured);
            if(p.Kind=="windows-pe-iso")secureToggle.IsCheckedChanged+=(_,_)=>parameterGroup.IsVisible=parameterInfo?.Editable==true&&secureToggle.IsChecked==true;
            secureMode=f.Choose(Tr("Boot method"),modes.Select(mode=>mode=="signed-ipxe"?"iPXE shim":Tr("Native network boot")).ToArray(),Array.IndexOf(modes,p.SecureBoot.Mode));
            f.Note(Strings.Message(p.SecureBoot.Detail));
            f.BeginAdvanced(Tr("View check results"));
            foreach(var c in p.SecureBoot.Components)f.Text(c.File,string.Join("\n",c.Signer,c.Issuer,"SHA-256: "+c.Sha256,c.CertificateSha256),true).IsReadOnly=true;
            f.EndAdvanced();
        }
        f.BeginAdvanced(Tr("Advanced settings"));
        var arch=f.Text(Tr("Architecture"),p.Architecture);arch.IsReadOnly=secured;
        var kindKeys=new[]{"linux","bsd","windows","windows-pe-iso","windows-ris","fit","raw","solaris"};
        var kind=f.Choose(Tr("Type"),kindKeys.Select(k=>k=="windows-pe-iso"?Tr("Windows PE ISO"):k).ToArray(),Array.IndexOf(kindKeys,p.Kind));kind.IsEnabled=!secured;
        if(source is null)kind.SelectionChanged+=(_,_)=>parameterGroup.IsVisible=kind.SelectedIndex>=0&&kindKeys[kind.SelectedIndex]!="windows-pe-iso";
        var root=f.Text(Tr("Directory"),p.Root);root.IsReadOnly=secured;
        var kernel=f.Text(Tr("Kernel / bootloader"),p.Kernel);kernel.IsReadOnly=secured;
        var initrd=f.Text("Initrd",p.Initrd);initrd.IsReadOnly=secured;
        var dtb=f.Text("DTB",p.Dtb);dtb.IsReadOnly=secured;
        var mapping=f.Text(Tr("Boot file mappings"),string.Join("\n",p.BootFiles.Select(k=>$"{k.Key}={k.Value}")),true);mapping.IsReadOnly=secured;
        if(p.BootEvidence is BootEvidence e){
            f.Section(Tr("Detection and check results"));
            foreach(var pair in new[]{(Tr("Entry"),e.Entry),(Tr("Boot configuration"),e.Config),(Tr("Kernel"),e.Kernel),("Initrd",e.Initrd),(Tr("Installation source"),SourceLabel(e.Source))})if(!string.IsNullOrEmpty(pair.Item2))f.Text(pair.Item1,pair.Item2).IsReadOnly=true;
            foreach(var diagnostic in (e.Diagnostics??[]).Where(ActionableDiagnostic))f.Text(Tr("Action required"),Strings.Message(diagnostic.Detail),true).IsReadOnly=true;
        }
        f.OnSave=async()=>{
          try {
            p.Name=name.Text??"";p.Arguments=args.Text??"";
            p.Architecture=arch.Text??"";p.Kind=kind.SelectedIndex>=0?kindKeys[kind.SelectedIndex]:"raw";p.Root=root.Text??"";p.Kernel=kernel.Text??"";p.Initrd=initrd.Text??"";p.Dtb=dtb.Text??"";
            p.BootFiles=(mapping.Text??"").Split('\n',StringSplitOptions.RemoveEmptyEntries).Select(line=>line.Split('=',2)).ToDictionary(parts=>parts[0].Trim(),parts=>parts.Length==2?parts[1].Trim():throw new Exception(Tr("Use x64-efi=bootx64.efi, one mapping per line")));
            if(source is null)await api.Send("/api/profiles",p);
            else await api.Send($"/api/profiles/{p.Id}/settings",new{profile=p,secure_boot=secureToggle is null?null:new{enabled=secureToggle.IsChecked==true,mode=secureMode is not null&&secureMode.SelectedIndex>=0?modes[secureMode.SelectedIndex]:p.SecureBoot?.Mode}},HttpMethod.Put);
            await Refresh();return true;
          }catch(Exception error){f.ShowError(Strings.Message(error.Message));return false;}
        };
        await f.ShowDialog<bool>(this);
    }
    async Task Network(){
        if(current==null)return;
        var settings=System.Text.Json.Nodes.JsonNode.Parse(current.Settings.GetRawText())!;
        var f=new FormWindow(Tr("Network settings"),Tr("Save")){Height=Math.Min(660,Math.Max(520,ClientSize.Height-40))};
        var result=new StackPanel{Spacing=8};
        var recheck=new Button{Content=Tr("Recheck")};
        void RenderReport(){
            result.Children.Clear();
            result.Children.Add(new TextBlock{Text="DHCP  ·  "+DhcpStatus(),TextWrapping=Avalonia.Media.TextWrapping.Wrap});
            result.Children.Add(new TextBlock{Text=Tr("Permissions & components")+"  ·  "+(permissions is null?Tr("Not checked"):permissions.Issues.Length>0?Tr("Needs attention"):Tr("Ready")),TextWrapping=Avalonia.Media.TextWrapping.Wrap});
            if(current?.Running==true)result.Children.Add(new TextBlock{Text=Tr("Stop services to recheck DHCP"),TextWrapping=Avalonia.Media.TextWrapping.Wrap});
            var issues=new[]{networkCheckError}.Concat(permissions?.Issues??[]).Where(x=>!string.IsNullOrWhiteSpace(x)).Select(x=>AccessGuidance.Describe(x!)).Distinct();
            foreach(var issue in issues){
                result.Children.Add(new TextBlock{Text=issue,TextWrapping=Avalonia.Media.TextWrapping.Wrap});
            }
            foreach(var url in NetworkIssues().Select(AccessGuidance.DownloadUrl).OfType<string>().Distinct()){
                var link=Action(Tr("Open official download page"),()=>{System.Diagnostics.Process.Start(new System.Diagnostics.ProcessStartInfo(url){UseShellExecute=true});return Task.CompletedTask;});
                result.Children.Add(link);
            }
            if(!string.IsNullOrWhiteSpace(current?.Network?.Detail))result.Children.Add(new Expander{Header=Tr("Check details"),HorizontalAlignment=HorizontalAlignment.Stretch,Content=new SelectableTextBlock{Text=AccessGuidance.Describe(current.Network.Detail),TextWrapping=Avalonia.Media.TextWrapping.Wrap}});
        }
        RenderReport();f.Add(result);f.Add(recheck);
        f.Section(Tr("Interface & addresses"));
        var choices=new[]{new NetworkChoice(null,null)}.Concat((current.Network?.Interfaces??[]).Where(i=>i.Mac.Length>0).SelectMany(i=>i.Addresses.Length==0?new[]{new NetworkChoice(i,null)}:i.Addresses.Select(a=>new NetworkChoice(i,a)))).ToArray();
        var selected=Array.FindIndex(choices,c=>c.Interface?.Id==settings["interface"]?.ToString()&&c.Address?.Ip==settings["server_ip"]?.ToString());
        if(selected<0&&!string.IsNullOrEmpty(settings["interface"]?.ToString())) {
            var saved=new NetworkInterfaceInfo(settings["interface"]!.ToString(),settings["interface"]!.ToString(),settings["server_mac"]!.ToString(),false,false,[],[],"");
            choices=choices.Append(new NetworkChoice(saved,new NetworkAddress(settings["server_ip"]!.ToString(),settings["subnet"]!.ToString()))).ToArray();selected=choices.Length-1;
        }
        var showSelector=current.Network?.ShowInterfaceSelector==true;
        var picker=showSelector?f.Choose(Tr("Interface and IPv4"),choices,selected):null;
        var address=f.Text(Tr("Server IPv4"),settings["server_ip"]!.ToString());
        var subnet=f.Text(Tr("Subnet mask"),settings["subnet"]!.ToString());
        f.Section(Tr("Boot services"));
        var automatic=f.Check(Tr("Detect DHCP automatically"),settings["dhcp_auto"]?.GetValue<bool>()??false);
        var dhcp=f.Check(Tr("Allocate addresses"),settings["dhcp_enabled"]!.GetValue<bool>());
        var proxy=f.Check(Tr("ProxyDHCP (external DHCP assigns addresses)"),settings["proxy_dhcp_enabled"]?.GetValue<bool>()??false);
        void Mode(){dhcp.IsEnabled=proxy.IsEnabled=automatic.IsChecked!=true;}
        automatic.IsCheckedChanged+=(_,_)=>Mode();Mode();
        dhcp.IsCheckedChanged+=(_,_)=>{if(dhcp.IsChecked==true)proxy.IsChecked=false;};proxy.IsCheckedChanged+=(_,_)=>{if(proxy.IsChecked==true)dhcp.IsChecked=false;};
        var start=new TextBox{Text=settings["pool_start"]!.ToString()};var end=new TextBox{Text=settings["pool_end"]!.ToString()};
        var pool=new StackPanel{Spacing=10,Children={new TextBlock{Text=Tr("Pool start")},start,new TextBlock{Text=Tr("Pool end")},end}};f.Add(pool);
        void PoolMode(){pool.IsVisible=dhcp.IsChecked==true && automatic.IsChecked!=true;}
        dhcp.IsCheckedChanged+=(_,_)=>PoolMode();automatic.IsCheckedChanged+=(_,_)=>PoolMode();PoolMode();
        f.BeginAdvanced(Tr("Advanced settings"));
        var bind=f.Text(Tr("Listen IPv4"),settings["bind_ip"]!.ToString());
        var iface=showSelector?f.Text(Tr("Network adapter"),settings["interface"]!.ToString()):new TextBox{Text=settings["interface"]!.ToString()};
        var mac=f.Text(Tr("Adapter MAC"),settings["server_mac"]!.ToString());
        var ethernet=f.Check(Tr("Ethernet protocols"),settings["ethernet_enabled"]!.GetValue<bool>());
        if(picker is not null)picker.SelectionChanged+=(_,_)=>{if(picker.SelectedItem is NetworkChoice c){iface.Text=c.Interface?.Id??"";if(c.Interface is not null){mac.Text=c.Interface.Mac;ethernet.IsChecked=true;}if(c.Address is not null){address.Text=bind.Text=c.Address.Ip;subnet.Text=c.Address.Subnet;}}};
        recheck.Click+=async(_,_)=>{
            f.SetBusy(true);recheck.IsEnabled=false;recheck.Content=Tr("Detecting…");
            try{await CheckNetwork(true);RenderReport();}
            finally{f.SetBusy(false);recheck.IsEnabled=true;recheck.Content=Tr("Recheck");}
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
    async Task Uboot(){if(systems.SelectedItem is not Profile p)return;var f=new FormWindow("U-Boot");var box=f.Text(Strings.Tr("Commands"),await api.Uboot(p.Id),true);box.IsReadOnly=true;await f.ShowDialog<bool>(this);}
    async Task Commands(){if(systems.SelectedItem is not Profile p)return;var result=await api.BootCommands(p.Id,Chinese?"zh":"en");var status=result.Commands.Length>0?Tr("Run these commands in the firmware"):result.Firmware is "pxe" or "efi"?Tr("Select network boot in the firmware menu; no command is needed"):result.Firmware=="openfirmware"?Tr("Use the network boot option in the firmware"):Tr("Complete the boot configuration using the notes below");await new CommandWindow(p.Name,result.Title,status,result.Commands,result.Notes,Tr("Copy commands"),Tr("Copied"),Tr("Close")).ShowDialog<bool>(this);}
    async Task About(){
        using var stream=typeof(MainWindow).Assembly.GetManifestResourceStream("Bootah.ThirdParty.json")!;
        var entries=System.Text.Json.JsonSerializer.Deserialize<ThirdPartyComponent[]>(stream,new System.Text.Json.JsonSerializerOptions{PropertyNameCaseInsensitive=true})!;
        var platform=OperatingSystem.IsWindows()?"windows":"linux";
        var window=new Window{Title=Tr("About & open-source licenses"),Width=740,Height=620,WindowStartupLocation=WindowStartupLocation.CenterOwner};
        var root=new DockPanel{Margin=new Thickness(24)};
        var title=new TextBlock{Text="Bootah",FontSize=26,FontWeight=Avalonia.Media.FontWeight.SemiBold,Margin=new Thickness(0,0,0,18)};DockPanel.SetDock(title,Dock.Top);root.Children.Add(title);
        CoreVersion? coreVersion=null;try{coreVersion=(await api.State()).CoreVersion;}catch{}
        var versions=new StackPanel{Spacing=4,Margin=new Thickness(0,0,0,12)};
        versions.Children.Add(new TextBlock{Text=$"{Tr("Client version")}: {ClientVersion.Display} ({ClientVersion.Build})"});
        versions.Children.Add(new TextBlock{Text=$"{Tr("Core version")}: {(coreVersion is null?Tr("Unavailable"):$"{coreVersion.Display} ({coreVersion.Build})")}"});
        DockPanel.SetDock(versions,Dock.Top);root.Children.Add(versions);
        var close=new Button{Content=Tr("Close"),HorizontalAlignment=HorizontalAlignment.Right,Margin=new Thickness(0,16,0,0)};close.Click+=(_,_)=>window.Close();DockPanel.SetDock(close,Dock.Bottom);root.Children.Add(close);
        Button Link(string text,Func<Task> action){var button=Action(text,action);button.Content=new TextBlock{Text=text,TextWrapping=Avalonia.Media.TextWrapping.Wrap,TextDecorations=Avalonia.Media.TextDecorations.Underline};button.Padding=new Thickness(0);button.Background=Avalonia.Media.Brushes.Transparent;button.BorderThickness=new Thickness(0);button.HorizontalAlignment=HorizontalAlignment.Left;return button;}
        Control LabeledLink(string label,string value,Func<Task> action){var row=new Grid{ColumnDefinitions=new ColumnDefinitions("Auto,*")};row.Children.Add(new TextBlock{Text=Tr(label)+":",Margin=new Thickness(0,0,6,0),VerticalAlignment=VerticalAlignment.Top});var link=Link(value,action);Grid.SetColumn(link,1);row.Children.Add(link);return row;}
        Task OpenSource(string url){System.Diagnostics.Process.Start(new System.Diagnostics.ProcessStartInfo(url){UseShellExecute=true});return Task.CompletedTask;}
        var list=new ListBox{ItemsSource=entries.Where(e=>e.Platforms.Contains(platform)).ToArray(),Background=Avalonia.Media.Brushes.Transparent,BorderThickness=new Thickness(0)};
        list.Styles.Add(new Avalonia.Styling.Style(x=>x.OfType<ListBoxItem>()){Setters={new Avalonia.Styling.Setter(ListBoxItem.HorizontalContentAlignmentProperty,HorizontalAlignment.Stretch)}});
        list.ItemTemplate=new Avalonia.Controls.Templates.FuncDataTemplate<ThirdPartyComponent>((entry,_)=>{
            if(entry is null)return new TextBlock();
            return new StackPanel{Spacing=5,Margin=new Thickness(0,10),Children={
                new TextBlock{Text=entry.Name+(entry.Version.Length>0?"  "+entry.Version:""),FontSize=16,FontWeight=Avalonia.Media.FontWeight.SemiBold},
                LabeledLink("Source",entry.Source,()=>OpenSource(entry.Source)),
                LabeledLink("License",Tr(entry.License),async()=>{if(entry.Notice.Length==0){await OpenSource(entry.LicenseUrl);return;}var viewer=new Window{Title=entry.Name+" — "+Tr("License"),Width=720,Height=560,WindowStartupLocation=WindowStartupLocation.CenterOwner};var panel=new DockPanel{Margin=new Thickness(20)};var done=new Button{Content=Tr("Close"),HorizontalAlignment=HorizontalAlignment.Right,Margin=new Thickness(0,12,0,0)};done.Click+=(_,_)=>viewer.Close();DockPanel.SetDock(done,Dock.Bottom);panel.Children.Add(done);panel.Children.Add(new TextBox{Text=entry.Notice,IsReadOnly=true,AcceptsReturn=true,TextWrapping=Avalonia.Media.TextWrapping.Wrap});viewer.Content=panel;await viewer.ShowDialog(window);})
            }};
        });root.Children.Add(list);window.Content=root;await window.ShowDialog(this);
    }

}
class FormWindow:Window {
    readonly StackPanel fields=new(){Spacing=10,Margin=new Thickness(22)};
    public Func<Task<bool>>? OnSave;
    readonly TextBlock error=new(){TextWrapping=Avalonia.Media.TextWrapping.Wrap};
    public void ShowError(string message){error.Text=message;}
    StackPanel? advanced;
    Control? buttons;
    StackPanel Fields=>advanced??fields;
    public FormWindow(string title,string? confirmLabel=null){fields.Children.Add(error);Title=title;Width=520;Height=600;WindowStartupLocation=WindowStartupLocation.CenterOwner;var root=new DockPanel();var bar=new StackPanel{Orientation=Orientation.Horizontal,HorizontalAlignment=HorizontalAlignment.Right,Spacing=8,Margin=new Thickness(20)};var cancel=new Button{Content=Strings.Tr("Cancel")};cancel.Click+=(_,_)=>Close(false);var save=new Button{Content=confirmLabel??Strings.Tr("OK")};save.Click+=async(_,_)=>{if(OnSave is null){Close(true);return;}SetBusy(true);try{if(await OnSave())Close(true);}finally{SetBusy(false);}};bar.Children.Add(cancel);bar.Children.Add(save);buttons=bar;DockPanel.SetDock(bar,Dock.Bottom);root.Children.Add(bar);root.Children.Add(new ScrollViewer{Content=fields});Content=root;}
    public TextBox Text(string label,string value,bool multiline=false){Fields.Children.Add(new TextBlock{Text=label});var box=new TextBox{Text=value,AcceptsReturn=multiline,MinHeight=multiline?110:32};Fields.Children.Add(box);return box;}
    public void Add(Control control)=>Fields.Children.Add(control);
    public void Section(string title)=>Fields.Children.Add(new TextBlock{Text=title,FontWeight=Avalonia.Media.FontWeight.SemiBold,Margin=new Thickness(0,14,0,0)});
    public void Note(string value){Fields.Children.Add(new TextBlock{Text=value,Opacity=.65,TextWrapping=Avalonia.Media.TextWrapping.Wrap});}
    public ComboBox Choose<T>(string label,T[] values,int selected){Fields.Children.Add(new TextBlock{Text=label});var box=new ComboBox{ItemsSource=values,SelectedIndex=Math.Max(0,selected),HorizontalAlignment=HorizontalAlignment.Stretch};Fields.Children.Add(box);return box;}
    public CheckBox Check(string label,bool value){var box=new CheckBox{Content=label,IsChecked=value};Fields.Children.Add(box);return box;}
    public void SetBusy(bool busy){if(buttons is not null)buttons.IsEnabled=!busy;}
    public StackPanel BeginGroup(){advanced=new StackPanel{Spacing=10};fields.Children.Add(advanced);return advanced;}
    public void EndAdvanced()=>advanced=null;
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
