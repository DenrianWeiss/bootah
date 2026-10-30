using Avalonia;
using Avalonia.Controls;
using Avalonia.Controls.ApplicationLifetimes;
using Avalonia.Themes.Fluent;

namespace Netboot;
class Program {
    [STAThread] public static void Main(string[] args) {
        var index=Array.IndexOf(args,"--api-token");
        App.ApiToken=index>=0 && index+1<args.Length ? args[index+1] : Environment.GetEnvironmentVariable("BOOTAH_API_TOKEN");
        AppBuilder.Configure<App>().UsePlatformDetect().StartWithClassicDesktopLifetime(args);
    }
}
class App : Application {
    internal static string? ApiToken;
    internal static string CoreBinary => Environment.GetEnvironmentVariable("NETBOOT_CORE")??System.IO.Path.Combine(AppContext.BaseDirectory,OperatingSystem.IsWindows()?"netboot-core.exe":"netboot-core");
    internal static string CoreConfig => Environment.GetEnvironmentVariable("NETBOOT_CONFIG")??System.IO.Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData),"Netboot","state.json");
    public override void Initialize() => Styles.Add(new FluentTheme());
    public override void OnFrameworkInitializationCompleted() {
        if (ApplicationLifetime is IClassicDesktopStyleApplicationLifetime desktop) {
            string? startupError=null;
            if(Environment.GetEnvironmentVariable("NETBOOT_API") is string endpoint && ApiToken is null)ApiToken=TokenStore.Load(endpoint);
            try {
            var binary=CoreBinary;
            if(Environment.GetEnvironmentVariable("NETBOOT_API") is null){
            ApiToken ??= Convert.ToHexString(System.Security.Cryptography.RandomNumberGenerator.GetBytes(32)).ToLowerInvariant();
            if(!System.IO.File.Exists(binary)) throw new FileNotFoundException("The Bootah core is missing or cannot be executed. Download and reinstall the complete Bootah package for this platform, or correct NETBOOT_CORE and its execute permissions.");
                var directory=System.IO.Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData),"Netboot");
                System.IO.Directory.CreateDirectory(directory);
                var start=new System.Diagnostics.ProcessStartInfo(binary){UseShellExecute=false,CreateNoWindow=true,RedirectStandardInput=true,RedirectStandardOutput=true,RedirectStandardError=true};
                start.ArgumentList.Add("--exit-on-stdin-close");
                start.ArgumentList.Add("--api-token-stdin");
                start.ArgumentList.Add("--config");start.ArgumentList.Add(CoreConfig);
                var core=System.Diagnostics.Process.Start(start);
                if(core is not null){core.StandardInput.Write(ApiToken+"\n");core.StandardInput.Flush();}
                if(core is not null){var sync=new object();var log=System.IO.Path.Combine(System.IO.Path.GetDirectoryName(CoreConfig)??directory,"core.log");void Record(string? line){if(line is null)return;lock(sync){try{if(System.IO.File.Exists(log)&&new System.IO.FileInfo(log).Length>4*1024*1024)System.IO.File.WriteAllText(log,"");System.IO.File.AppendAllText(log,line+Environment.NewLine);}catch(System.IO.IOException){}}}core.OutputDataReceived+=(_,e)=>Record(e.Data);core.ErrorDataReceived+=(_,e)=>Record(e.Data);core.BeginOutputReadLine();core.BeginErrorReadLine();}
                desktop.Exit+=(_,_)=>{if(core is not null){try{core.StandardInput.Close();if(!core.WaitForExit(2000))core.Kill(true);}catch(InvalidOperationException){}finally{core.Dispose();}}};
            }
            } catch(Exception e){startupError=(e is System.ComponentModel.Win32Exception or UnauthorizedAccessException ? "The Bootah core is missing or cannot be executed. Download and reinstall the complete Bootah package for this platform, or correct NETBOOT_CORE and its execute permissions."+"\n" : "")+e.Message;}
            var window=new MainWindow();
            if(startupError is not null) window.Opened+=async (_,_)=>await window.ShowFailure(startupError);
            desktop.MainWindow = window;
        }
        base.OnFrameworkInitializationCompleted();
    }
}
