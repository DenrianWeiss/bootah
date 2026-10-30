package work.premutation.bootah

import android.app.*
import android.content.Intent
import android.graphics.drawable.Icon
import android.net.wifi.WifiManager
import android.os.IBinder
import android.os.PowerManager
import android.widget.Toast
import java.io.File

class CoreService : Service() {
    private var core:Process?=null
    private var rootMode=false
    private var wake:PowerManager.WakeLock?=null
    private var multicast:WifiManager.MulticastLock?=null
    override fun onCreate(){
        super.onCreate()
        val manager=getSystemService(NotificationManager::class.java)
        manager.createNotificationChannel(NotificationChannel("netboot","Bootah",NotificationManager.IMPORTANCE_LOW))
        val stop=PendingIntent.getService(this,0,Intent(this,CoreService::class.java).setAction("stop"),PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT)
        startForeground(1,Notification.Builder(this,"netboot").setContentTitle("Bootah").setSmallIcon(R.drawable.ic_bootah).addAction(Notification.Action.Builder(Icon.createWithResource(this,android.R.drawable.ic_media_pause),"停止",stop).build()).build())
        wake=getSystemService(PowerManager::class.java).newWakeLock(PowerManager.PARTIAL_WAKE_LOCK,"netboot:server").apply{setReferenceCounted(false);acquire()}
        multicast=(applicationContext.getSystemService(WIFI_SERVICE)as WifiManager).createMulticastLock("netboot").apply{setReferenceCounted(false);acquire()}
    }
    override fun onStartCommand(intent:Intent?,flags:Int,startId:Int):Int {
        if(intent?.action=="stop"){stopSelf();return START_NOT_STICKY}
        val root=intent?.getBooleanExtra("root",false)==true
        if(core?.isAlive==true){if(root!=rootMode)Toast.makeText(this,"请先结束本机服务",Toast.LENGTH_SHORT).show();return START_NOT_STICKY}
        val binary=File(applicationInfo.nativeLibraryDir,"libnetboot_core.so")
        if(!binary.exists()){Toast.makeText(this,"缺少服务程序",Toast.LENGTH_LONG).show();stopSelf();return START_NOT_STICKY}
        val config=File(filesDir,"state.json")
        if(!config.exists())config.writeText("""{"settings":{"dhcp_auto":true,"dhcp_enabled":false,"tftp_port":${if(root)69 else 1069},"http_port":8080}}""")
        val bootAssets=File(filesDir,"boot-assets").apply{mkdirs()}
        assets.list("boot")?.forEach{name->assets.open("boot/$name").use{input->File(bootAssets,name).outputStream().use{input.copyTo(it)}}}
        val token=intent?.getStringExtra("api_token") ?: ApiCredentials.newToken()
        val command=listOf(binary.path,"--exit-on-stdin-close","--api-token-stdin","--config",config.path)+if(intent?.getBooleanExtra("serve",false)==true)listOf("--serve")else emptyList()
        try{
            val process=if(root)ProcessBuilder("su","-c","NETBOOT_ASSETS="+bootAssets.path+" exec "+command.joinToString(" "){"'"+it.replace("'","'\\''")+"'"}).redirectErrorStream(true).start()else ProcessBuilder(command).apply{environment()["NETBOOT_ASSETS"]=bootAssets.path}.redirectErrorStream(true).start()
            process.outputStream.write((token+"\n").toByteArray());process.outputStream.flush();ApiCredentials.localToken=token
            core=process;rootMode=root
            Thread{try{process.inputStream.use{input->var log=File(filesDir,"core.log").outputStream();try{val buffer=ByteArray(8192);var size=0;while(true){val count=input.read(buffer);if(count<0)break;if(size+count>4*1024*1024){log.close();log=File(filesDir,"core.log").outputStream();size=0};log.write(buffer,0,count);size+=count}}finally{log.close()}}}catch(_:java.io.IOException){}finally{stopSelf(startId)}}.start()
        }catch(e:Exception){File(filesDir,"core.log").writeText(e.message.orEmpty());Toast.makeText(this,e.message,Toast.LENGTH_LONG).show();stopSelf()}
        return START_NOT_STICKY
    }
    override fun onBind(intent:Intent?):IBinder?=null
    override fun onDestroy(){
        val process=core
        ApiCredentials.localToken=null
        Thread{try{process?.outputStream?.close();if(process!=null&&!process.waitFor(3,java.util.concurrent.TimeUnit.SECONDS))process.destroy()}catch(_:Exception){process?.destroy()}}.start()
        multicast?.let{if(it.isHeld)it.release()};wake?.let{if(it.isHeld)it.release()}
        super.onDestroy()
    }
}
