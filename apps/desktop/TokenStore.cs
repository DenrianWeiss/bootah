using System.Diagnostics;
using System.Runtime.InteropServices;
using System.Security.Cryptography;
using System.Text;

namespace Netboot;
static class TokenStore {
    [StructLayout(LayoutKind.Sequential)] struct Blob {public int Size;public IntPtr Data;}
    [DllImport("crypt32.dll",SetLastError=true,CharSet=CharSet.Unicode)] static extern bool CryptProtectData(ref Blob input,string? description,IntPtr entropy,IntPtr reserved,IntPtr prompt,int flags,out Blob output);
    [DllImport("crypt32.dll",SetLastError=true)] static extern bool CryptUnprotectData(ref Blob input,IntPtr description,IntPtr entropy,IntPtr reserved,IntPtr prompt,int flags,out Blob output);
    [DllImport("kernel32.dll")] static extern IntPtr LocalFree(IntPtr memory);
    static byte[] Transform(byte[] bytes,bool encrypt){
        var input=new Blob{Size=bytes.Length,Data=Marshal.AllocHGlobal(bytes.Length)};Marshal.Copy(bytes,0,input.Data,bytes.Length);Blob output=default;
        try {if(!(encrypt?CryptProtectData(ref input,null,IntPtr.Zero,IntPtr.Zero,IntPtr.Zero,1,out output):CryptUnprotectData(ref input,IntPtr.Zero,IntPtr.Zero,IntPtr.Zero,IntPtr.Zero,1,out output)))throw new System.ComponentModel.Win32Exception();var result=new byte[output.Size];Marshal.Copy(output.Data,result,0,output.Size);return result;}
        finally{Marshal.FreeHGlobal(input.Data);if(output.Data!=IntPtr.Zero)LocalFree(output.Data);}
    }
    static string PathFor(string endpoint){var name=Convert.ToHexString(SHA256.HashData(Encoding.UTF8.GetBytes(endpoint)));return System.IO.Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData),"Netboot","credentials",name);}
    static Process Start(string verb,string endpoint){var info=new ProcessStartInfo("secret-tool"){UseShellExecute=false,RedirectStandardInput=true,RedirectStandardOutput=true,RedirectStandardError=true,CreateNoWindow=true};info.ArgumentList.Add(verb);if(verb=="store")info.ArgumentList.Add("--label=Bootah API");info.ArgumentList.Add("service");info.ArgumentList.Add("bootah-api");info.ArgumentList.Add("endpoint");info.ArgumentList.Add(endpoint);return Process.Start(info)??throw new IOException("Credential service unavailable");}
    public static string? Load(string endpoint){try{if(OperatingSystem.IsWindows()){var path=PathFor(endpoint);return File.Exists(path)?Encoding.UTF8.GetString(Transform(File.ReadAllBytes(path),false)):null;}if(OperatingSystem.IsLinux()){using var process=Start("lookup",endpoint);process.StandardInput.Close();var result=process.StandardOutput.ReadToEnd().TrimEnd('\r','\n');process.WaitForExit();return process.ExitCode==0&&!string.IsNullOrEmpty(result)?result:null;}}catch(Exception){}return null;}
    public static bool Save(string endpoint,string token){try{if(OperatingSystem.IsWindows()){var path=PathFor(endpoint);Directory.CreateDirectory(System.IO.Path.GetDirectoryName(path)!);File.WriteAllBytes(path,Transform(Encoding.UTF8.GetBytes(token),true));return true;}if(OperatingSystem.IsLinux()){using var process=Start("store",endpoint);process.StandardInput.Write(token);process.StandardInput.Close();process.WaitForExit();return process.ExitCode==0;}}catch(Exception){}return false;}
    public static void Remove(string endpoint){try{if(OperatingSystem.IsWindows())File.Delete(PathFor(endpoint));else if(OperatingSystem.IsLinux()){using var process=Start("clear",endpoint);process.StandardInput.Close();process.WaitForExit();}}catch(Exception){}}
}
