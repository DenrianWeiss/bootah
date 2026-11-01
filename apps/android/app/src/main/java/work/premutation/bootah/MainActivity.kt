package work.premutation.bootah

import android.app.*
import android.os.Bundle
import android.content.Intent
import android.content.Context
import android.graphics.Typeface
import android.view.Gravity
import android.view.View
import android.widget.*
import org.json.*
import java.net.HttpURLConnection
import java.net.URL
import java.util.concurrent.Executors

class MainActivity : Activity() {
    private val worker = Executors.newSingleThreadExecutor()
    private lateinit var layout: LinearLayout
    private lateinit var logToolbar: LinearLayout
    private lateinit var contentScroll: ScrollView
    private lateinit var content: LinearLayout
    private lateinit var state: TextView
    private lateinit var toggle: Button
    private var snapshot = JSONObject()
    private val automationCapabilities=mutableMapOf<String,List<String>>()
    private val capabilitySignatures=mutableMapOf<String,String>()
    private val capabilityRequests=mutableSetOf<String>()
    private var page = "Devices"
    private var selectedDeviceId:String?=null
    private var selectedProfileId:String?=null
    private var deletingSelection=false
    private lateinit var deleteSelectionButton:Button
    private var trustLocalIso=false
    private var pendingIsoTrust=false
    private var pendingAutomation:Triple<String,String,String>?=null
    private var pendingAutomationNext:(()->Unit)?=null
    private var exportLogLevel="info"
    private var exportRaw=false
    private var isoTrustCheck:CheckBox?=null
    private lateinit var heading:TextView
    private val navigation=mutableMapOf<String,Button>()
    private val dark get()=resources.configuration.uiMode and android.content.res.Configuration.UI_MODE_NIGHT_MASK == android.content.res.Configuration.UI_MODE_NIGHT_YES
    private val localeTag get()=resources.configuration.locales.get(0).toLanguageTag()
    private val chinese get()=localeTag.startsWith("zh")
    private fun tr(key:String)=Strings.tr(key,localeTag)
    private fun message(text:String)=Strings.message(text,localeTag)
    private val ink get()=android.graphics.Color.parseColor(if(dark)"#F1F3F7" else "#18212E")
    private val muted get()=android.graphics.Color.parseColor(if(dark)"#A7B1BF" else "#657181")
    private val surface get()=android.graphics.Color.parseColor(if(dark)"#202630" else "#FFFFFF")
    private val canvasColor get()=android.graphics.Color.parseColor(if(dark)"#141920" else "#F3F5F8")
    private fun dp(value:Int)=(value*resources.displayMetrics.density).toInt()
    private fun shape(color:Int)=android.graphics.drawable.GradientDrawable().apply{setColor(color);cornerRadius=dp(16).toFloat()}

    private var permissionsChecked=false
    private var refreshing=false
    private var networkSignature=""
    private var detecting=false
    private var startAfterDetection=false
    private var coreRequested=false
    private var pendingConflictId:String?=null
    private fun localCore()=try{URL(endpoint).host in setOf("127.0.0.1","localhost","::1")}catch(_:Exception){false}
    private val handler=android.os.Handler(android.os.Looper.getMainLooper())
    private val poll=object:Runnable{override fun run(){refresh();handler.postDelayed(this,3000)}}
    private val endpoint get() = getPreferences(0).getString("endpoint", "http://127.0.0.1:9080")!!
    private var sessionToken:String?=null
    private fun token():String? = sessionToken ?: (if(localCore()) ApiCredentials.localToken else null) ?: ApiCredentials.load(this,endpoint)
    private fun authorize(connection:HttpURLConnection){connection.instanceFollowRedirects=false;token()?.let{connection.setRequestProperty("Authorization","Bearer $it")}}
    @Suppress("DEPRECATION")
    override fun onCreate(savedInstanceState: Bundle?) {
        setTheme(if(dark)android.R.style.Theme_Material_NoActionBar else android.R.style.Theme_Material_Light_NoActionBar)
        super.onCreate(savedInstanceState)
        layout = LinearLayout(this).apply {orientation=LinearLayout.VERTICAL;setBackgroundColor(canvasColor)}
        val padding=dp(20)
        layout.setOnApplyWindowInsetsListener{view,insets->
            if(android.os.Build.VERSION.SDK_INT>=30){val appearance=android.view.WindowInsetsController.APPEARANCE_LIGHT_STATUS_BARS or android.view.WindowInsetsController.APPEARANCE_LIGHT_NAVIGATION_BARS;window.insetsController?.setSystemBarsAppearance(if(dark)0 else appearance,appearance)}
            else window.decorView.systemUiVisibility=if(dark)0 else View.SYSTEM_UI_FLAG_LIGHT_STATUS_BAR or View.SYSTEM_UI_FLAG_LIGHT_NAVIGATION_BAR
            if(android.os.Build.VERSION.SDK_INT>=30){val bars=insets.getInsets(android.view.WindowInsets.Type.systemBars());view.setPadding(bars.left,bars.top,bars.right,bars.bottom)}
            else view.setPadding(insets.systemWindowInsetLeft,insets.systemWindowInsetTop,insets.systemWindowInsetRight,insets.systemWindowInsetBottom)
            insets
        }
        val header=LinearLayout(this).apply{gravity=android.view.Gravity.CENTER_VERTICAL;setPadding(padding,dp(12),padding,dp(12))}
        header.addView(TextView(this).apply {text="Bootah";textSize=22f;setTextColor(ink);setTypeface(null,Typeface.BOLD)},LinearLayout.LayoutParams(0,-2,1f))
        state=TextView(this).apply{textSize=11f;setTextColor(muted);isSingleLine=true;ellipsize=android.text.TextUtils.TruncateAt.END;includeFontPadding=false;gravity=android.view.Gravity.CENTER_VERTICAL;setPadding(padding,0,padding,0)}
        val settingsButton=ImageButton(this).apply {
            setImageResource(R.drawable.ic_settings)
            imageTintList=android.content.res.ColorStateList.valueOf(ink)
            val backgroundValue=android.util.TypedValue()
            theme.resolveAttribute(android.R.attr.selectableItemBackgroundBorderless,backgroundValue,true)
            setBackgroundResource(backgroundValue.resourceId)
            contentDescription=tr("Settings");tooltipText=contentDescription
            scaleType=ImageView.ScaleType.CENTER_INSIDE;setPadding(dp(12),dp(12),dp(12),dp(12))
            setOnClickListener{settings()}
        }
        header.addView(settingsButton,LinearLayout.LayoutParams(dp(48),dp(48)));layout.addView(header)
        val bar=LinearLayout(this).apply{gravity=android.view.Gravity.CENTER_VERTICAL;setPadding(padding,dp(12),padding,dp(12))}
        heading=TextView(this).apply{textSize=30f;setTextColor(ink);setTypeface(null,Typeface.BOLD)};bar.addView(heading,LinearLayout.LayoutParams(0,-2,1f))
        toggle=button(tr("Start"),primary=true) {if(snapshot.length()==0){startAfterDetection=true;if(localCore())startForegroundService(Intent(this,CoreService::class.java))}else if(snapshot.optBoolean("running")) request("/api/service/stop","POST",JSONObject()){refresh()} else detectNetwork { request("/api/service/start","POST",JSONObject()){refresh()} }}
        deleteSelectionButton=button(tr("Delete")){deleteSelection()}
        bar.addView(deleteSelectionButton);bar.addView(toggle);layout.addView(bar)
        logToolbar=LinearLayout(this).apply{orientation=LinearLayout.VERTICAL;setPadding(padding,0,padding,0)};layout.addView(logToolbar)
        content=LinearLayout(this).apply {orientation=LinearLayout.VERTICAL;setPadding(padding,dp(8),padding,dp(24))}
        contentScroll=ScrollView(this).apply {isFillViewport=true;addView(content)}
        layout.addView(contentScroll,LinearLayout.LayoutParams(-1,0,1f))
        val tabs=LinearLayout(this).apply{setPadding(dp(12),dp(8),dp(12),dp(8));setBackgroundColor(surface)}
        listOf("Devices","Image library","Activity").forEach {name->val item=button(tabName(name)){page=name;render()};navigation[name]=item;tabs.addView(item,LinearLayout.LayoutParams(0,dp(64),1f))}
        layout.addView(state,LinearLayout.LayoutParams(-1,dp(24)))
        layout.addView(tabs)
        setContentView(layout);render();refresh()
    }
    private fun button(label:String, primary:Boolean=false, action:()->Unit)=Button(this,null,0,if(primary)android.R.style.Widget_Material_Button_Colored else android.R.style.Widget_Material_Button_Borderless).apply {text=label;isAllCaps=false;setOnClickListener{action()}}
    private fun tabName(name:String)=when(name){"Devices"->tr("Devices");"Image library"->tr("Images");else->tr("Activity")}
    private fun actions(vararg items:Pair<String,()->Unit>) {
        val bar=LinearLayout(this).apply{orientation=LinearLayout.HORIZONTAL}
        items.forEach { (label,action)->bar.addView(button(label,action=action),LinearLayout.LayoutParams(-2,-2).apply{marginEnd=dp(8)}) }
        content.addView(bar,LinearLayout.LayoutParams(-1,-2).apply{bottomMargin=dp(12)})
    }
    private fun logoId(profile:JSONObject):String = when(val id=profile.optString("system_id")){
        "windows11","windows"->"windows11-original"
        "windows10","windows8","windows7","windowsserver"->"windows8-original"
        "kylin","uos"->"linux"
        ""->if(profile.optString("kind")=="linux")"linux" else ""
        else->if(id in setOf("ubuntu","debian","fedora","archlinux","linuxmint","nixos","alpinelinux","opensuse","centos","redhat","rockylinux","almalinux","linux","aosc","freebsd","netbsd","openbsd","solaris","windowsxp","windowsserver2003"))id else ""
    }
    private fun row(title:String,detail:String,badge:String="",icon:String?=null,selected:Boolean=false,longAction:(()->Unit)?=null,action:()->Unit){
        val card=LinearLayout(this).apply{orientation=LinearLayout.VERTICAL;setPadding(dp(18),dp(18),dp(18),dp(18));background=android.graphics.drawable.RippleDrawable(android.content.res.ColorStateList.valueOf(0x180066CC),shape(if(selected)android.graphics.Color.parseColor(if(dark)"#33445D" else "#DCE8F8") else surface),null);isSelected=selected;setOnClickListener{action()};if(longAction!=null)setOnLongClickListener{longAction();true}}
        val titleRow=LinearLayout(this).apply{orientation=LinearLayout.HORIZONTAL;gravity=android.view.Gravity.CENTER_VERTICAL}
        if(!icon.isNullOrBlank())try{val theme=if(dark)"dark" else "light";val bitmap=assets.open("system-icons/$theme/$icon.png").use{android.graphics.BitmapFactory.decodeStream(it)};titleRow.addView(ImageView(this).apply{setImageBitmap(bitmap);scaleType=ImageView.ScaleType.FIT_CENTER;setPadding(dp(5),dp(5),dp(5),dp(5))},LinearLayout.LayoutParams(dp(42),dp(42)).apply{marginEnd=dp(12)})}catch(_:Exception){}
        titleRow.addView(TextView(this).apply{text=title;textSize=17f;setTextColor(ink);setTypeface(null,Typeface.BOLD)});card.addView(titleRow)
        if(detail.isNotBlank())card.addView(TextView(this).apply{text=detail;textSize=13f;setTextColor(muted);setPadding(0,dp(6),0,0)})
        if(badge.isNotBlank())card.addView(TextView(this).apply{text=badge;textSize=13f;maxLines=1;ellipsize=android.text.TextUtils.TruncateAt.MIDDLE;tooltipText=badge;setTextColor(android.graphics.Color.parseColor(if(dark)"#9FC5FF" else "#2563AF"));setPadding(0,dp(14),0,0)})
        content.addView(card,LinearLayout.LayoutParams(-1,-2).apply{bottomMargin=dp(10)})
    }
    private fun request(path:String, method:String="GET", body:JSONObject?=null, quiet:Boolean=false, finished:()->Unit={}, failed:(Int?)->Unit={}, failedBody:((Int,String)->Unit)?=null, done:(String)->Unit={}) {
        worker.execute {
            try {
                val connection=URL(endpoint+path).openConnection() as HttpURLConnection
                authorize(connection)
                connection.requestMethod=method;connection.connectTimeout=5000;connection.readTimeout=if(path=="/api/media/import"||(path.endsWith("/compatibility")||path.contains("/secure-boot")))1800000 else if(path=="/api/service/start"||path=="/api/network/detect")150000 else if(path=="/api/network/permissions")40000 else 10000
                if(body!=null){connection.doOutput=true;connection.setRequestProperty("Content-Type","application/json");connection.outputStream.use{it.write(body.toString().toByteArray())}}
                val code=connection.responseCode
                val ok=code in 200..299 && connection.getHeaderField("X-Bootah-Authentication")=="bearer-v1"
                val text=if(code in 200..299&&!ok)tr("The core did not confirm token authentication") else (if(ok)connection.inputStream else connection.errorStream)?.bufferedReader()?.use{it.readText()} ?: ""
                connection.disconnect()
                runOnUiThread{if(ok)done(text)else{failed(code);failedBody?.invoke(code,text);if(!quiet)Toast.makeText(this,text.lines().joinToString("\n"){message(it)},Toast.LENGTH_LONG).show()}}
            }catch(e:Exception){runOnUiThread{failed(null);state.text=tr("Disconnected");if(!quiet)Toast.makeText(this,message(e.message.orEmpty()),Toast.LENGTH_LONG).show()}}
            finally{runOnUiThread{finished()}}
        }
    }
    @Suppress("DEPRECATION")
    private fun networkEvidence():JSONObject {
        if(!localCore())return JSONObject()
        val cm=getSystemService(Context.CONNECTIVITY_SERVICE) as android.net.ConnectivityManager
        val active=cm.activeNetwork ?: return JSONObject()
        val caps=cm.getNetworkCapabilities(active) ?: return JSONObject()
        if(!caps.hasTransport(android.net.NetworkCapabilities.TRANSPORT_WIFI)&&!caps.hasTransport(android.net.NetworkCapabilities.TRANSPORT_ETHERNET))return JSONObject()
        val props=cm.getLinkProperties(active) ?: return JSONObject()
        val name=props.interfaceName ?: return JSONObject()
        val addresses=JSONArray()
        props.linkAddresses.filter{it.address is java.net.Inet4Address}.forEach { a->
            val mask=if(a.prefixLength==0)0L else (0xffffffffL shl (32-a.prefixLength)) and 0xffffffffL
            val subnet=(3 downTo 0).joinToString("."){((mask shr (8*it)) and 255).toString()}
            addresses.put(JSONObject().put("ip",a.address.hostAddress).put("subnet",subnet))
        }
        var server:String?=null
        if(android.os.Build.VERSION.SDK_INT>=30)server=props.dhcpServerAddress?.hostAddress
        else if(caps.hasTransport(android.net.NetworkCapabilities.TRANSPORT_WIFI)) {
            val info=(applicationContext.getSystemService(Context.WIFI_SERVICE) as android.net.wifi.WifiManager).dhcpInfo
            if(info!=null&&info.serverAddress!=0)server=(0..3).joinToString("."){((info.serverAddress ushr (8*it)) and 255).toString()}
        }
        val mac=try{java.net.NetworkInterface.getByName(name)?.hardwareAddress?.joinToString(":"){"%02x".format(it.toInt() and 255)}?:""}catch(_:Exception){""}
        val evidence=JSONObject().put("id",name).put("name",name).put("mac",mac).put("up",true).put("physical",true).put("addresses",addresses).put("dhcp_servers",JSONArray().apply{if(server!=null)put(server)}).put("source","android")
        return JSONObject().put("interface",name).put("android",evidence)
    }
    private fun detectNetwork(after:()->Unit={}) {
        if(detecting)return
        detecting=true
        val evidence=networkEvidence();networkSignature=evidence.toString()
        request("/api/network/detect","POST",evidence,finished={detecting=false},failed={networkSignature=""}) {refresh();if(!permissionsChecked)checkPermissions();after();if(startAfterDetection){startAfterDetection=false;request("/api/service/start","POST",JSONObject()){refresh()}}}
    }
    private fun checkPermissions(force:Boolean=false){
        permissionsChecked=true
        request("/api/network/permissions",quiet=true,failed={showPermissionIssues(listOf(tr("Network permission checks could not complete. Check the core connection and retry from network settings.")))}){text->
            val issues=JSONObject(text).optJSONArray("issues")?:JSONArray()
            val messages=(0 until issues.length()).map{issues.getString(it).lines().joinToString("\n"){line->tr(line)}}.toMutableList()
            if(localCore()&&android.os.Build.VERSION.SDK_INT>=33&&checkSelfPermission(android.Manifest.permission.POST_NOTIFICATIONS)!=android.content.pm.PackageManager.PERMISSION_GRANTED)messages.add(tr("Notifications are disabled. Allow Bootah notifications in app settings to see background service status."))
            if(messages.isNotEmpty())showPermissionIssues(messages)else if(force)AlertDialog.Builder(this).setTitle(tr("Network permissions")).setMessage(tr("No additional authorization required")).setPositiveButton(tr("OK"),null).show()
        }
    }
    private fun showPermissionIssues(messages:List<String>){
        if(isFinishing||isDestroyed)return
        val dialog=AlertDialog.Builder(this).setTitle(tr("Permissions requiring attention")).setMessage(messages.joinToString("\n\n")).setPositiveButton(tr("Settings")){_,_->settings()}.setNegativeButton(tr("Later"),null)
        if(localCore()&&android.os.Build.VERSION.SDK_INT>=33&&checkSelfPermission(android.Manifest.permission.POST_NOTIFICATIONS)!=android.content.pm.PackageManager.PERMISSION_GRANTED)dialog.setNeutralButton(tr("Open app settings")){_,_->startActivity(Intent(android.provider.Settings.ACTION_APP_NOTIFICATION_SETTINGS).putExtra(android.provider.Settings.EXTRA_APP_PACKAGE,packageName))}
        dialog.show()
    }
    private fun refresh(){if(refreshing)return;refreshing=true;request("/api/state",quiet=true,finished={refreshing=false},failed={code->if(code==401)state.text=tr("Management token missing or invalid") else if(code!=null && code in 200..299)state.text=tr("The core did not confirm token authentication") else if(localCore()&&!coreRequested){coreRequested=true;startForegroundService(Intent(this,CoreService::class.java))}}){
        snapshot=JSONObject(it)
        loadAutomationCapabilities()
        val result=snapshot.optJSONObject("network")
        val detection=result?.optString("status").orEmpty()
        val fullStatus=tr(if(snapshot.optBoolean("running"))"Running" else "Stopped")+"  "+tr(detection)
        state.contentDescription=fullStatus
        state.text=tr(if(snapshot.optBoolean("running"))"Running" else "Stopped")+"  "+tr(when(detection){"lease_found"->"DHCP: system record";"response_found"->"DHCP: live response";else->detection})
        toggle.text=tr(if(snapshot.optBoolean("running"))"Stop" else "Start");render();checkDeviceConflict()
        if(!snapshot.optBoolean("running")&&!detecting&&networkEvidence().toString()!=networkSignature)detectNetwork()
        else if(!detecting&&!permissionsChecked)checkPermissions()
    }}
    private fun checkDeviceConflict(){
        val conflict=snapshot.optJSONArray("device_conflicts")?.optJSONObject(0)
        if(conflict==null){pendingConflictId=null;return}
        val id=conflict.optString("id");if(id==pendingConflictId)return;pendingConflictId=id
        val details=if(conflict.optString("reason")=="ip-mac")"IP ${conflict.optString("ip")}: ${conflict.optString("old_mac")} → ${conflict.optString("new_mac")}" else "${conflict.optString("new_mac")}: ${conflict.optString("old_architecture")} → ${conflict.optString("new_architecture")}"
        val message=details+"\n"+tr("The old device has manual settings. After removal, the new device will use its default image on the next network boot.")
        val resolve:(Boolean)->Unit={accept->request("/api/device-conflicts/$id","POST",JSONObject().put("accept",accept)){refresh()}}
        AlertDialog.Builder(this).setTitle(tr("Remove the old device record?")).setMessage(message).setPositiveButton(tr("Remove")){_,_->resolve(true)}.setNegativeButton(tr("Keep")){_,_->resolve(false)}.setCancelable(false).show()
    }
    private fun deleteSelection(){
        val devices=page=="Devices"
        val id=(if(devices)selectedDeviceId else selectedProfileId)?:return
        deletingSelection=true;deleteSelectionButton.isEnabled=false
        request("/api/${if(devices)"devices" else "profiles"}/$id","DELETE",finished={deletingSelection=false;render()}){
            if(devices)selectedDeviceId=null else selectedProfileId=null
            refresh()
        }
    }
    private fun render(){
        fun exists(key:String,id:String?):Boolean {val entries=snapshot.optJSONArray(key)?:return false;return (0 until entries.length()).any{entries.getJSONObject(it).optString("id")==id}}
        if(!exists("devices",selectedDeviceId))selectedDeviceId=null
        if(!exists("profiles",selectedProfileId))selectedProfileId=null
        deleteSelectionButton.visibility=if(page=="Activity")View.GONE else View.VISIBLE
        deleteSelectionButton.isEnabled=!deletingSelection && (if(page=="Devices")selectedDeviceId!=null else selectedProfileId!=null)
        logToolbar.removeAllViews();logToolbar.visibility=if(page=="Activity")View.VISIBLE else View.GONE
        content.removeAllViews();heading.text=when(page){"Devices"->tr("Devices");"Image library"->tr("Image library");else->tr("Activity")}
        for((name,item)in navigation){item.setTextColor(if(name==page)android.graphics.Color.parseColor(if(dark)"#AFCFFF" else "#245EA7") else muted);item.isSelected=name==page}
        when(page){
            "Devices"->{actions(tr("Add device") to {device(JSONObject())});val devices=snapshot.optJSONArray("devices");if(devices==null||devices.length()==0)row(tr("No devices yet"),"",""){device(JSONObject())};devices?.each {d->val profiles=snapshot.optJSONArray("profiles");var assigned=tr("No image assigned");val defaultId=snapshot.optJSONObject("default_profiles")?.optString(d.optString("architecture"));profiles?.each{if(it.optString("id")==d.optString("profile_id"))assigned=it.optString("name") else if(d.isNull("profile_id")&&it.optString("id")==defaultId)assigned=tr("Assign by architecture")+"  "+it.optString("name")};row(d.optString("name").ifEmpty{d.optString("mac").ifEmpty{d.optString("ip")}},listOf(d.optString("ip"),d.optString("architecture"),d.optString("model")).filter{it.isNotBlank()&&it!="null"}.joinToString("  "),assigned,selected=d.optString("id")==selectedDeviceId,longAction={deviceMenu(d)}){selectedDeviceId=d.optString("id");render()}}}
            "Image library"->{architectureDefaults();actions(tr("Import ISO") to {pickIso()},tr("Advanced manual setup") to {profile(JSONObject())});isoTrustCheck=CheckBox(this).apply{text=tr("Trust selected local ISO source");isChecked=trustLocalIso;setTextColor(ink);setOnCheckedChangeListener{_,checked->trustLocalIso=checked}};content.addView(isoTrustCheck);snapshot.optJSONArray("profiles")?.each {p->row(p.optString("system_name").ifBlank{p.optString("name")},"${p.optString("name")}  ${p.optString("architecture")}  ${p.optString("kind")}",(if(p.optString("status")=="needs-attention")tr("Needs setup") else p.optString("media")),logoId(p),selected=p.optString("id")==selectedProfileId,longAction={profileMenu(p)}){selectedProfileId=p.optString("id");render()}}}
            else->{val logBar=LinearLayout(this).apply{orientation=LinearLayout.VERTICAL;setPadding(0,dp(4),0,dp(12))};val filterRow=LinearLayout(this).apply{orientation=LinearLayout.HORIZONTAL;gravity=Gravity.CENTER_VERTICAL};filterRow.addView(button(tr("Log level"),action={chooseLogLevel()}),LinearLayout.LayoutParams(-2,-2).apply{marginEnd=dp(16)});filterRow.addView(CheckBox(this).apply{text=tr("Include private data");isChecked=exportRaw;setTextColor(ink);setOnCheckedChangeListener{_,checked->exportRaw=checked}},LinearLayout.LayoutParams(0,-2,1f));logBar.addView(filterRow);logBar.addView(button(tr("Export diagnostics"),action={saveDiagnostics()}),LinearLayout.LayoutParams(-2,-2).apply{topMargin=dp(12);gravity=Gravity.END});val logActions=LinearLayout(this).apply{orientation=LinearLayout.HORIZONTAL};logActions.addView(button(tr("Clear logs")){request("/api/events","DELETE"){refresh()}},LinearLayout.LayoutParams(0,-2,1f));logActions.addView(button(tr("Back to top")){contentScroll.smoothScrollTo(0,0)},LinearLayout.LayoutParams(0,-2,1f));logBar.addView(logActions);logToolbar.addView(logBar);val events=snapshot.optJSONArray("events");if(events==null||events.length()==0)row(tr("No activity yet"),"",""){};events?.let{for(i in it.length()-1 downTo 0){val e=it.getJSONObject(i);row(e.optString("protocol"),e.optString("message"),e.optString("device")) {}}}}
        }
    }
    private fun chooseLogLevel(){val levels=arrayOf(tr("Errors"),tr("Standard"),tr("Detailed"));val values=arrayOf("error","info","debug");AlertDialog.Builder(this).setTitle(tr("Log level")).setSingleChoiceItems(levels,values.indexOf(exportLogLevel)){dialog,index->exportLogLevel=values[index];dialog.dismiss()}.show()}
    private fun saveDiagnostics(){
        val start={startActivityForResult(Intent(Intent.ACTION_CREATE_DOCUMENT).apply{addCategory(Intent.CATEGORY_OPENABLE);type="text/plain";putExtra(Intent.EXTRA_TITLE,"bootah-diagnostics.txt")},42)}
        if(exportRaw)AlertDialog.Builder(this).setTitle(tr("Raw logs contain private data")).setMessage(tr("Raw logs may contain device addresses and local file paths.")).setNegativeButton(tr("Cancel"),null).setPositiveButton(tr("Continue")){_,_->start()}.show() else start()
    }
    private fun exportDiagnostics(uri:android.net.Uri){
        val level=exportLogLevel;val raw=exportRaw
        worker.execute{try{val binary=java.io.File(applicationInfo.nativeLibraryDir,"libnetboot_core.so");val config=java.io.File(filesDir,"state.json");val command=mutableListOf(binary.path,"--export-diagnostics","--config",config.path,"--log-level",level);if(raw)command.add("--include-private");val process=ProcessBuilder(command).start();val data=process.inputStream.use{it.readBytes()};val errors=process.errorStream.bufferedReader().use{it.readText()};if(process.waitFor()!=0)throw java.io.IOException(errors);contentResolver.openOutputStream(uri,"w")?.use{it.write(data)}?:throw java.io.IOException("Cannot open destination");runOnUiThread{Toast.makeText(this,tr("Diagnostics exported"),Toast.LENGTH_LONG).show()}}catch(e:Exception){runOnUiThread{Toast.makeText(this,e.message?:tr("Export failed"),Toast.LENGTH_LONG).show()}}}
    }


    private fun loadAutomationCapabilities(){
        snapshot.optJSONArray("profiles")?.each{p->val id=p.getString("id");val signature=endpoint+p.toString();if(capabilitySignatures[id]!=signature&&!capabilityRequests.contains(id)){
            capabilityRequests.add(id);automationCapabilities.remove(id)
            request("/api/profiles/$id/automation-methods",quiet=true,finished={capabilityRequests.remove(id)}){text->val array=JSONArray(text);automationCapabilities[id]=(0 until array.length()).map{array.getString(it)};capabilitySignatures[id]=signature}
        }}
    }
    private fun automationSupported(profileId:String?)=profileId!=null&&automationCapabilities[profileId]?.isNotEmpty()==true
    private fun deviceProfile(d:JSONObject):String?=d.optString("profile_id").takeIf{it.isNotBlank()&&it!="null"}?:snapshot.optJSONObject("default_profiles")?.optString(d.optString("architecture"))?.takeIf{it.isNotBlank()}
    private fun itemMenu(title:String,labels:Array<String>,enabled:BooleanArray,choose:(Int)->Unit){
        val adapter=object:ArrayAdapter<String>(this,android.R.layout.simple_list_item_1,labels){
            override fun areAllItemsEnabled()=false
            override fun isEnabled(position:Int)=enabled[position]
            override fun getView(position:Int,convertView:View?,parent:android.view.ViewGroup):View=super.getView(position,convertView,parent).apply{isEnabled=enabled[position];alpha=if(enabled[position])1f else .4f}
        }
        AlertDialog.Builder(this).setTitle(title).setAdapter(adapter){_,which->if(enabled[which])choose(which)}.show()
    }
    private fun deviceMenu(d:JSONObject){itemMenu(d.optString("name").ifBlank{d.optString("mac")},arrayOf(tr("Edit"),tr("Automatic installation")),booleanArrayOf(true,automationSupported(deviceProfile(d)))){which->if(which==0)device(d) else chooseAutomation("device",d.optString("id"))}}

    private fun pickIso(){pendingIsoTrust=trustLocalIso;trustLocalIso=false;isoTrustCheck?.isChecked=false;startActivityForResult(Intent(Intent.ACTION_OPEN_DOCUMENT).apply{type="*/*";addCategory(Intent.CATEGORY_OPENABLE);putExtra(Intent.EXTRA_MIME_TYPES,arrayOf("application/x-iso9660-image","application/octet-stream"))},40)}
    @Deprecated("Framework activity result compatibility")
    override fun onActivityResult(requestCode:Int,resultCode:Int,data:Intent?){super.onActivityResult(requestCode,resultCode,data);if(requestCode==41){val pending=pendingAutomation;val next=pendingAutomationNext;pendingAutomation=null;pendingAutomationNext=null;if(resultCode==RESULT_OK&&pending!=null&&data?.data!=null)uploadAutomation(pending,data.data!!,next?:{} ) else next?.invoke();return};if(resultCode!=RESULT_OK)return;val uri=data?.data?:return
        if(requestCode==42){exportDiagnostics(uri);return}
        if(requestCode!=40)return
        if(endpoint!="http://127.0.0.1:9080"){Toast.makeText(this,tr("ISO import requires a local service"),Toast.LENGTH_LONG).show();return}
        importUri(uri,pendingIsoTrust)
    }
    private fun importUri(uri:android.net.Uri,trustSelectedIso:Boolean=false){
        state.text=tr("Importing…")
        worker.execute{
            val directory=java.io.File(filesDir,"imports/${java.util.UUID.randomUUID()}")
            var submitted=false
            try{
                check(directory.mkdirs()) { "Cannot create ISO import directory" }
                var name="installer.iso"
                contentResolver.query(uri,arrayOf(android.provider.OpenableColumns.DISPLAY_NAME),null,null,null)?.use{if(it.moveToFirst())name=it.getString(0)}
                val iso=java.io.File(directory,name.replace(Regex("[^a-zA-Z0-9._-]"),"_").let{if(it.isBlank()||it=="."||it=="..")"installer.iso" else it})
                contentResolver.openInputStream(uri)!!.use{input->iso.outputStream().use{input.copyTo(it)}}
                submitted=true
                runOnUiThread{
                    request("/api/media/import","POST",JSONObject().put("path",iso.absolutePath).put("trust_local_iso",trustSelectedIso),failed={code->
                        if(code!=null&&code in 400..499)directory.deleteRecursively()
                        else Toast.makeText(this,tr("Import outcome is uncertain; ISO retained to preserve boot references."),Toast.LENGTH_LONG).show()
                    }){text->
                        // Profiles read this original ISO on demand after import.
                        val result=JSONObject(text);refresh()
                        val warnings=result.optJSONArray("warnings")
                        val notes=(0 until (warnings?.length()?:0)).map{warnings!!.getString(it)}
                        Toast.makeText(this,tr("ISO imported"),Toast.LENGTH_SHORT).show()
                        if(notes.isNotEmpty())AlertDialog.Builder(this).setTitle(tr("Action required")).setMessage(notes.map(::message).joinToString("\n\n")).setPositiveButton(tr("OK"),null).show()
                    }
                }
            }catch(e:Exception){
                if(!submitted)directory.deleteRecursively()
                runOnUiThread{Toast.makeText(this,message(e.message.orEmpty()),Toast.LENGTH_LONG).show()}
            }
        }

    }
    private fun chooseAutomation(scope:String,id:String,after:()->Unit={}){
        val profileId=if(scope=="profile")id else {val device=snapshot.optJSONArray("devices")?.let{items->(0 until items.length()).map{items.getJSONObject(it)}.firstOrNull{it.optString("id")==id}};device?.optString("profile_id")?.takeIf{it.isNotBlank()&&it!="null"}?:snapshot.optJSONObject("default_profiles")?.optString(device?.optString("architecture"))?:""}
        if(profileId.isBlank()){after();return}
        request("/api/profiles/$profileId/automation-methods"){response->val array=JSONArray(response);val methods=(0 until array.length()).map{array.getString(it)};if(methods.isEmpty()){after();return@request};val choices=listOf(tr("No automatic configuration"))+methods;AlertDialog.Builder(this).setTitle(tr("Automatic installation")).setItems(choices.toTypedArray()){_,index->if(index==0){request("/api/automation/$scope/$id","DELETE"){refresh();after()}}else{pendingAutomation=Triple(scope,id,methods[index-1]);pendingAutomationNext=after;startActivityForResult(Intent(Intent.ACTION_OPEN_DOCUMENT).apply{type="*/*";addCategory(Intent.CATEGORY_OPENABLE)},41)}}.setNegativeButton(tr("Later")){_,_->after()}.show()}
    }
    private fun uploadAutomation(pending:Triple<String,String,String>,uri:android.net.Uri,next:()->Unit={}){worker.execute{try{val name=contentResolver.query(uri,arrayOf(android.provider.OpenableColumns.DISPLAY_NAME),null,null,null)?.use{if(it.moveToFirst())it.getString(0) else "config"}?:"config";val address="$endpoint/api/automation/${pending.first}/${pending.second}?method=${java.net.URLEncoder.encode(pending.third,"UTF-8")}&filename=${java.net.URLEncoder.encode(name,"UTF-8")}";val connection=URL(address).openConnection() as HttpURLConnection;authorize(connection);connection.requestMethod="PUT";connection.doOutput=true;connection.connectTimeout=5000;connection.readTimeout=30000;contentResolver.openInputStream(uri)!!.use{input->connection.outputStream.use{input.copyTo(it)}};val code=connection.responseCode;val error=if(code in 200..299 && connection.getHeaderField("X-Bootah-Authentication")!="bearer-v1")tr("The core did not confirm token authentication") else if(code !in 200..299)connection.errorStream?.bufferedReader()?.use{it.readText()}?.let{tr(it)} else null;connection.disconnect();runOnUiThread{if(error!=null)Toast.makeText(this,error,Toast.LENGTH_LONG).show();refresh();next()}}catch(e:Exception){runOnUiThread{Toast.makeText(this,e.message?.let{tr(it)}?:tr("Request failed"),Toast.LENGTH_LONG).show();next()}}}}
    private fun dialog(title:String,fields:List<Pair<String,String>>,checks:List<Pair<String,Boolean>> = emptyList(),save:(Map<String,String>)->Unit){
        val panel=LinearLayout(this).apply{orientation=LinearLayout.VERTICAL;setPadding(32,12,32,12)}
        val inputs=fields.map{(label,value)->val input=EditText(this).apply{hint=tr(label);setText(value);isSingleLine=label!="Boot file mappings"};panel.addView(input);label to input}.toMap()
        val toggles=checks.map{(label,value)->val input=CheckBox(this).apply{text=tr(label);isChecked=value};panel.addView(input);label to input}.toMap()
        AlertDialog.Builder(this).setTitle(title).setView(ScrollView(this).apply{addView(panel)}).setNegativeButton(tr("Cancel"),null).setPositiveButton(tr("Save")){_,_->save(inputs.mapValues{it.value.text.toString()}+toggles.mapValues{it.value.isChecked.toString()})}.show()
    }
    private fun device(d:JSONObject){
        val panel=LinearLayout(this).apply{orientation=LinearLayout.VERTICAL;setPadding(dp(24),dp(8),dp(24),dp(8))}
        panel.addView(TextView(this).apply{text=tr("Enter a MAC or IP address");setTextColor(muted);textSize=13f})
        val inputs=listOf("MAC" to "mac","IP" to "ip","Name" to "name","Model" to "model").associate{(label,key)->panel.addView(TextView(this).apply{text=tr(label);textSize=12f;setTextColor(muted);setPadding(0,dp(12),0,0)});val edit=EditText(this).apply{isSingleLine=true;setText(d.optString(key).takeIf{it!="null"}.orEmpty())};panel.addView(edit);key to edit}
        val profiles=snapshot.optJSONArray("profiles")?:JSONArray();val defaultId=snapshot.optJSONObject("default_profiles")?.optString(d.optString("architecture"));val defaultName=(0 until profiles.length()).map{profiles.getJSONObject(it)}.firstOrNull{it.optString("id")==defaultId}?.optString("name");val names=mutableListOf(defaultName?.let{tr("Assign by architecture")+"  "+it}?:tr("No image assigned"));profiles.each{names.add(it.optString("name"))};panel.addView(TextView(this).apply{text=tr("Boot image");setTextColor(muted);textSize=12f;setPadding(0,dp(16),0,0)})
        val picker=Spinner(this);picker.adapter=ArrayAdapter(this,android.R.layout.simple_spinner_dropdown_item,names);for(i in 0 until profiles.length())if(profiles.getJSONObject(i).optString("id")==d.optString("profile_id"))picker.setSelection(i+1);panel.addView(picker)
        AlertDialog.Builder(this).setTitle(tr("Devices")).setView(ScrollView(this).apply{addView(panel)}).setNegativeButton(tr("Cancel"),null).setPositiveButton(tr("Save")){_,_->if(inputs["mac"]?.text.isNullOrBlank()&&inputs["ip"]?.text.isNullOrBlank()){Toast.makeText(this,tr("Enter a MAC or IP address"),Toast.LENGTH_LONG).show();return@setPositiveButton};for((key,edit)in inputs)d.put(key,if(key=="ip"&&edit.text.isBlank())JSONObject.NULL else edit.text.toString());d.put("profile_id",if(picker.selectedItemPosition==0)JSONObject.NULL else profiles.getJSONObject(picker.selectedItemPosition-1).getString("id"));request("/api/devices","POST",d){refresh()}}.show()
    }
    private fun profile(original:JSONObject){
        if(original.has("id")&&original.optJSONObject("secure_boot")?.optBoolean("eligible")==true){
            request("/api/profiles/${original.getString("id")}/secure-boot"){text->val copy=JSONObject(original.toString());copy.put("secure_boot",JSONObject(text));profileForm(copy)}
        } else profileForm(original)
    }
    private fun profileForm(original:JSONObject){
        val p=JSONObject(original.toString())
        val existing=p.has("id")
        val support=p.optJSONObject("secure_boot")
        val secured=support?.optBoolean("enabled")==true
        val panel=LinearLayout(this).apply{orientation=LinearLayout.VERTICAL;setPadding(dp(24),dp(8),dp(24),dp(8))}
        var targetPanel=panel
        fun note(text:String){targetPanel.addView(TextView(this).apply{this.text=text;setTextIsSelectable(true);setTextColor(muted);setPadding(0,dp(10),0,0)})}
        fun input(label:String,value:String,editable:Boolean=true,multiline:Boolean=false):EditText {
            note(label)
            return EditText(this).apply{setText(value);setTextColor(ink);isSingleLine=!multiline;isEnabled=editable;targetPanel.addView(this)}
        }
        val name=input(tr("Name"),p.optString("name"))
        if(p.optString("media").isNotBlank())note(tr("Image file")+"\n"+p.optString("media"))
        var parameterEditable=!existing
        var parameterEnabled=secured
        val parameterPanel=LinearLayout(this).apply{orientation=LinearLayout.VERTICAL;visibility=if(existing)View.GONE else View.VISIBLE};panel.addView(parameterPanel);targetPanel=parameterPanel
        if(p.optString("kind")=="windows")note(tr("Windows options: bootlog=on sos=on; use entry={GUID} for multiple boot entries."))
        val args=input(tr("Boot parameters"),p.optString("arguments"),true,true)
        if(existing){
            var importedArguments:String?=null
            val reset=Button(this).apply{text=tr("Restore imported parameters");isEnabled=false;setOnClickListener{args.setText(importedArguments)}};parameterPanel.addView(reset)
            val preview=EditText(this).apply{isFocusable=false;setTextIsSelectable(true);visibility=View.GONE;minLines=3}
            parameterPanel.addView(Button(this).apply{text=tr("Preview parameters");setOnClickListener{val draft=JSONObject(p.toString()).put("arguments",args.text.toString());request("/api/profiles/${p.getString("id")}/parameters","POST",draft){text->preview.setText(JSONObject(text).optString("preview"));preview.visibility=View.VISIBLE}}});parameterPanel.addView(preview)
            request("/api/profiles/${p.getString("id")}/parameters"){text->val info=JSONObject(text);importedArguments=if(info.isNull("original_arguments"))null else info.optString("original_arguments");reset.isEnabled=importedArguments!=null&&info.optBoolean("editable");parameterEditable=info.optBoolean("editable");parameterPanel.visibility=if(parameterEditable&&(p.optString("kind")!="windows-pe-iso"||parameterEnabled))View.VISIBLE else View.GONE;}
        }
        targetPanel=panel
        var toggle:android.widget.Switch?=null
        var picker:Spinner?=null
        val modes=support?.optJSONArray("modes")?:JSONArray()
        if(support?.optBoolean("eligible")==true){
            toggle=android.widget.Switch(this).apply{text=tr("Secure Boot support");isChecked=secured;panel.addView(this)}
            if(p.optString("kind")=="windows-pe-iso")toggle.setOnCheckedChangeListener{_,enabled->parameterEnabled=enabled;parameterPanel.visibility=if(parameterEditable&&enabled)View.VISIBLE else View.GONE}
            picker=Spinner(this).apply{adapter=ArrayAdapter(this@MainActivity,android.R.layout.simple_spinner_dropdown_item,(0 until modes.length()).map{if(modes.getString(it)=="signed-ipxe")"iPXE shim" else tr("Native network boot")});setSelection((0 until modes.length()).firstOrNull{modes.getString(it)==support.optString("mode")}?:0);panel.addView(this)}
            note(message(support.optString("detail")))
            support.optJSONArray("components")?.each{c->note(listOf("file","signer","issuer","sha256","certificate_sha256").joinToString("\n"){c.optString(it)})}
        }
        val advanced=LinearLayout(this).apply{orientation=LinearLayout.VERTICAL;visibility=if(existing)View.GONE else View.VISIBLE}
        panel.addView(Button(this).apply{text=tr("Advanced settings");setOnClickListener{advanced.visibility=if(advanced.visibility==View.VISIBLE)View.GONE else View.VISIBLE}})
        val fields=linkedMapOf<String,EditText>()
        for((key,label,default)in listOf(Triple("architecture",tr("Architecture"),"arm64"),Triple("kind",tr("Type"),"linux"),Triple("root",tr("Directory"),""),Triple("kernel",tr("Kernel"),""),Triple("initrd","Initrd",""),Triple("dtb","DTB",""))){
            advanced.addView(TextView(this).apply{text=label;setTextColor(muted)})
            val edit=EditText(this).apply{setText(p.optString(key,default));isEnabled=!secured;setTextColor(ink)};advanced.addView(edit);fields[key]=edit
        }
        advanced.addView(TextView(this).apply{text=tr("Boot file mappings");setTextColor(muted)})
        val mapping=EditText(this).apply{setText(p.optJSONObject("boot_files")?.let{f->f.keys().asSequence().sorted().joinToString("\n"){"$it=${f.getString(it)}"}}.orEmpty());isEnabled=!secured;minLines=3};advanced.addView(mapping);panel.addView(advanced)
        p.optJSONObject("boot_evidence")?.let{e->
            note(tr("Detection and check results"))
            for((key,label)in listOf("config" to tr("Boot configuration"),"entry" to tr("Entry"),"kernel" to tr("Kernel"),"initrd" to "Initrd"))if(e.optString(key).isNotBlank())note(label+"\n"+e.optString(key))
            val source=when(e.optString("source")){"http-tree"->"HTTP installation tree";"http-iso"->"HTTP ISO";"nfs-tree"->"Read-only NFS file tree";"nbd"->"Read-only NBD ISO";"external"->"External installation source";"local-media"->"Local media required";else->"Unrecognized installation source"};note(tr("Installation source")+"\n"+tr(source))
            e.optJSONArray("diagnostics")?.each{d->if(d.optString("code") !in setOf("BOOT_CONFIG_FAMILY_LAYOUT","UOS_MANUAL_ONLY"))note(tr("Action required")+"\n"+message(d.optString("detail")))}
        }
        val dialog=AlertDialog.Builder(this).setTitle(tr("Image settings")).setView(ScrollView(this).apply{addView(panel)}).setNegativeButton(tr("Cancel"),null).setPositiveButton(tr("Save"),null).create()
        dialog.setOnShowListener{dialog.getButton(AlertDialog.BUTTON_POSITIVE).setOnClickListener{
            val files=JSONObject()
            for(line in mapping.text.toString().lines().filter{it.isNotBlank()}){
                val pair=line.split('=',limit=2)
                if(pair.size!=2||pair[0].isBlank()||pair[1].isBlank()||files.has(pair[0].trim())){mapping.error=tr("Use x64-efi=bootx64.efi, one mapping per line");return@setOnClickListener}
                files.put(pair[0].trim(),pair[1].trim())
            }
            p.put("id",p.optString("id",java.util.UUID.randomUUID().toString())).put("name",name.text.toString()).put("arguments",args.text.toString()).put("boot_files",files)
            for((key,edit)in fields)p.put(key,edit.text.toString())
            val body=if(existing)JSONObject().put("profile",p).put("secure_boot",if(toggle==null)JSONObject.NULL else JSONObject().put("enabled",toggle!!.isChecked).put("mode",if(modes.length()>0)modes.getString(picker!!.selectedItemPosition) else JSONObject.NULL)) else p
            dialog.getButton(AlertDialog.BUTTON_POSITIVE).isEnabled=false
            request(if(existing)"/api/profiles/${p.getString("id")}/settings" else "/api/profiles",if(existing)"PUT" else "POST",body,failed={dialog.getButton(AlertDialog.BUTTON_POSITIVE).isEnabled=true}){dialog.dismiss();refresh()}
        }}
        dialog.show()
    }
    private fun profileMenu(p:JSONObject){
        val labels=arrayOf(tr("Image settings"),tr("Automatic installation"),tr("Boot options"),tr("Delete"))
        itemMenu(p.optString("name"),labels,booleanArrayOf(true,automationSupported(p.optString("id")),true,true)){index->when(index){0->profile(p);1->chooseAutomation("profile",p.getString("id"));2->manual(p);3->request("/api/profiles/${p.getString("id")}","DELETE"){refresh()}}}
    }
    private val firmwareKeys=setOf("x86-bios","ia64","ia32-efi","x64-efi","ebc-efi","alpha","sparc64","powerpc-of","powerpc-epapr","power-opal","arm32-efi","arm64-efi","ia32-http","x64-http","arm32-http","arm64-http","arm32-uboot","arm64-uboot","riscv32-efi","riscv64-http","loongarch64-efi","riscv64-efi","hp-rmp")
    private fun architectureDefaults(){
        content.addView(TextView(this).apply{text=tr("Default boot images by architecture");setTextColor(ink);textSize=18f})
        val defaults=snapshot.optJSONObject("default_profiles")?:JSONObject()
        val profiles=snapshot.optJSONArray("profiles")?:JSONArray()
        for(key in (0 until profiles.length()).flatMap{profiles.getJSONObject(it).optJSONObject("boot_files")?.keys()?.asSequence()?.toList().orEmpty()}.filter{it in firmwareKeys}.distinct().sorted()){
            val candidates=(0 until profiles.length()).map{profiles.getJSONObject(it)}.filter{it.optJSONObject("boot_files")?.has(key)==true}
            val name=candidates.firstOrNull{it.optString("id")==defaults.optString(key)}?.optString("name")?:tr("Not configured")
            row(key,"",name){
                val labels=(listOf(tr("Not configured"))+candidates.map{it.optString("name")}).toTypedArray()
                val selected=candidates.indexOfFirst{it.optString("id")==defaults.optString(key)}+1
                AlertDialog.Builder(this).setTitle(key).setSingleChoiceItems(labels,selected){dialog,index->
                    val changed=JSONObject((snapshot.optJSONObject("default_profiles")?:JSONObject()).toString())
                    if(index==0)changed.remove(key) else changed.put(key,candidates[index-1].getString("id"))
                    dialog.dismiss();request("/api/default-profiles","PUT",changed){refresh()}
                }.setNegativeButton(tr("Cancel"),null).show()
            }
        }
    }
    private fun manual(p:JSONObject){
        request("/api/profiles/${p.getString("id")}/boot-commands?lang=${if(chinese)"zh" else "en"}"){response->
            val result=JSONObject(response)
            val commands=result.optString("commands")
            val notesArray=result.optJSONArray("notes")
            val notes=(0 until (notesArray?.length()?:0)).mapNotNull{notesArray?.optString(it)}.map(::message).joinToString("\n")
            val status=tr(if(commands.isNotBlank())"Run these commands in the firmware" else if(result.optString("firmware") in setOf("pxe","efi"))"Select network boot in the firmware menu; no command is needed" else if(result.optString("firmware")=="openfirmware")"Use the network boot option in the firmware" else "Complete the boot configuration using the notes below")
            val panel=LinearLayout(this).apply{orientation=LinearLayout.VERTICAL;setPadding(dp(24),dp(8),dp(24),dp(8))}
            panel.addView(TextView(this).apply{text=status;textSize=13f;setTextColor(muted)})
            if(commands.isNotBlank())panel.addView(TextView(this).apply{text=commands;typeface=Typeface.MONOSPACE;textSize=13f;setTextColor(ink);setTextIsSelectable(true);setPadding(0,dp(12),0,0)})
            if(notes.isNotBlank())panel.addView(TextView(this).apply{text=notes;textSize=13f;setTextColor(muted);setPadding(0,dp(12),0,0)})
            val dialog=AlertDialog.Builder(this).setTitle(result.optString("title")).setView(ScrollView(this).apply{addView(panel)}).setNegativeButton(tr("Close"),null)
            if(commands.isNotBlank())dialog.setPositiveButton(tr("Copy commands")){_,_->(getSystemService(Context.CLIPBOARD_SERVICE) as android.content.ClipboardManager).setPrimaryClip(android.content.ClipData.newPlainText("Boot commands",commands))}
            dialog.show()
        }
    }
    private fun settings(){
        AlertDialog.Builder(this).setTitle(tr("Settings")).setItems(arrayOf("Connection","Network","Start local service","Start with root","Stop local service","About & open-source licenses","Check permissions again").map(::tr).plus(tr("Windows installation source")).toTypedArray()){_,index->
            if(index==0)connectionSettings()
            else when(index){1->network();2->startForegroundService(Intent(this,CoreService::class.java));3->startForegroundService(Intent(this,CoreService::class.java).putExtra("root",true));4->stopService(Intent(this,CoreService::class.java));5->about();6->checkPermissions(true);7->configureSmb()}
        }.show()
    }
    private fun connectionSettings(){
        val panel=LinearLayout(this).apply{orientation=LinearLayout.VERTICAL;setPadding(dp(24),dp(12),dp(24),0)}
        val address=EditText(this).apply{hint=tr("Service address");setText(endpoint);isSingleLine=true}
        val secret=EditText(this).apply{hint=tr("Management token");inputType=android.text.InputType.TYPE_CLASS_TEXT or android.text.InputType.TYPE_TEXT_VARIATION_PASSWORD;setText(if(localCore()&&ApiCredentials.localToken!=null)"" else token().orEmpty())}
        val save=CheckBox(this).apply{text=tr("Save token securely");isChecked=true}
        panel.addView(address);panel.addView(secret);panel.addView(save)
        AlertDialog.Builder(this).setTitle(tr("Connection")).setView(panel).setNegativeButton(tr("Cancel"),null).setPositiveButton(tr("Save")){_,_->
            val url=address.text.toString().trim().trimEnd('/');val value=secret.text.toString().trim();getPreferences(0).edit().putString("endpoint",url).apply();sessionToken=value.ifEmpty{null}
            if(save.isChecked&&value.isNotEmpty())try{ApiCredentials.save(this,url,value)}catch(_:Exception){ApiCredentials.remove(this,url);Toast.makeText(this,tr("Could not save the token securely; it will be used for this session only"),Toast.LENGTH_LONG).show()} else ApiCredentials.remove(this,url)
            permissionsChecked=false;refresh()
        }.show()
    }
    private var changingSmb=false
    private fun configureSmb(){
        if(changingSmb)return
        changingSmb=true
        request("/api/smb",failed={changingSmb=false}){text->
            val state=JSONObject(text)
            val modes=arrayOf("off","auto","smb1","smb2")
            var selected=modes.indexOf(state.optString("configured_mode",state.optString("mode"))).coerceAtLeast(0)
            val options=arrayOf(tr("Off"),tr("Automatic (by image)"),"SMB1 — XP / Server 2003","SMB2 — Vista / Server 2008+")
            AlertDialog.Builder(this).setTitle(tr("Windows installation source")+" — "+state.optString("mode"))
                .setSingleChoiceItems(options,selected){_,index->selected=index}
                .setPositiveButton(tr("Apply")){_,_->switchSmb(modes[selected])}
                .setNegativeButton(tr("Cancel")){_,_->changingSmb=false}
                .setOnCancelListener{changingSmb=false}.show()
        }
    }
    private fun switchSmb(mode:String,expected:String?=null){
        val body=JSONObject().put("mode",mode).put("confirm",expected!=null)
        if(expected!=null)body.put("expected_mode",expected)
        request("/api/smb","POST",body,quiet=true,failed={code->
            if(code==null){changingSmb=false;Toast.makeText(this,tr("Could not reach the installation source service. Retry."),Toast.LENGTH_LONG).show()}
        },failedBody={code,text->
            val conflict=try{JSONObject(text)}catch(_:Exception){JSONObject()}
            if(code==409&&conflict.optString("code")=="smb_switch_confirmation"){
                AlertDialog.Builder(this).setTitle(tr("Stop the current installation source?"))
                    .setMessage(conflict.optString(if(chinese)"message_zh" else "message")+"\n"+tr("Windows installations in progress may be interrupted."))
                    .setPositiveButton(if(mode=="off")tr("Stop service") else tr("Stop and switch")){_,_->switchSmb(mode,conflict.getString("current_mode"))}
                    .setNegativeButton(tr("Cancel")){_,_->changingSmb=false}
                    .setOnCancelListener{changingSmb=false}.show()
            }else{changingSmb=false;Toast.makeText(this,text,Toast.LENGTH_LONG).show()}
        }){changingSmb=false;refresh()}
    }
    private fun about(){request("/api/state",quiet=true,failed={showAbout(null)}){text->showAbout(JSONObject(text).optJSONObject("core_version"))}}
    private fun showAbout(coreVersion:JSONObject?){
        val inventory=JSONArray(assets.open("third-party.json").bufferedReader().use{it.readText()})
        val entries=(0 until inventory.length()).map{inventory.getJSONObject(it)}.filter{entry->entry.getJSONArray("platforms").let{p->(0 until p.length()).any{p.getString(it)=="android"}}}
        fun link(label:String,action:()->Unit)=TextView(this).apply{text=label;setTextColor(android.graphics.Color.parseColor(if(dark)"#AFCFFF" else "#245EA7"));textSize=13f;paintFlags=paintFlags or android.graphics.Paint.UNDERLINE_TEXT_FLAG;setPadding(0,dp(4),0,dp(4));isClickable=true;isFocusable=true;setOnClickListener{action()}}
        fun labeledLink(label:String,value:String,action:()->Unit)=LinearLayout(this).apply{
            orientation=LinearLayout.HORIZONTAL
            addView(TextView(this@MainActivity).apply{text=tr(label)+": ";setTextColor(ink);textSize=13f;setPadding(0,dp(4),0,dp(4))})
            addView(link(value,action),LinearLayout.LayoutParams(0,-2,1f))
        }
        fun open(url:String){startActivity(Intent(Intent.ACTION_VIEW,android.net.Uri.parse(url)))}
        val adapter=object:BaseAdapter(){
            override fun getCount()=entries.size
            override fun getItem(position:Int)=entries[position]
            override fun getItemId(position:Int)=position.toLong()
            override fun getView(position:Int,convertView:View?,parent:android.view.ViewGroup):View {
                val entry=entries[position]
                return LinearLayout(this@MainActivity).apply{
                    orientation=LinearLayout.VERTICAL;setPadding(dp(24),dp(12),dp(24),dp(12))
                    addView(TextView(this@MainActivity).apply{text=entry.getString("name")+entry.optString("version").let{if(it.isBlank())"" else "  $it"};textSize=16f;setTextColor(ink);setTypeface(null,Typeface.BOLD)})
                    addView(labeledLink("Source",entry.getString("source")){open(entry.getString("source"))})
                    addView(labeledLink("License",tr(entry.getString("license"))){
                        val notice=entry.getString("notice")
                        if(notice.isEmpty())open(entry.getString("licenseUrl")) else {
                            val text=TextView(this@MainActivity).apply{setText(notice);textSize=13f;setTextColor(ink);setTextIsSelectable(true);setPadding(dp(24),dp(16),dp(24),dp(16))}
                            AlertDialog.Builder(this@MainActivity).setTitle(entry.getString("name")+" — "+tr("License")).setView(ScrollView(this@MainActivity).apply{addView(text)}).setPositiveButton(tr("Close"),null).show()
                        }
                    })
                }
            }
        }
        val header=LinearLayout(this).apply{orientation=LinearLayout.VERTICAL;setPadding(dp(24),dp(12),dp(24),dp(12))
            addView(TextView(this@MainActivity).apply{text="${tr("Client version")}: ${BuildConfig.VERSION_NAME} (${BuildConfig.VERSION_CODE})";setTextColor(ink)})
            addView(TextView(this@MainActivity).apply{text="${tr("Core version")}: "+(coreVersion?.let{"${it.optString("display")} (${it.optInt("build")})"}?:tr("Unavailable"));setTextColor(ink)})}
        AlertDialog.Builder(this).setTitle(tr("About & open-source licenses")).setView(ListView(this).apply{addHeaderView(header,null,false);setAdapter(adapter);divider=null}).setPositiveButton(tr("Close"),null).show()
    }
    private fun network(){
        val settings=snapshot.optJSONObject("settings")?:return
        val fields=linkedMapOf("Server IPv4" to "server_ip","Listen IPv4" to "bind_ip","Pool start" to "pool_start","Pool end" to "pool_end","Subnet mask" to "subnet","DHCP port" to "dhcp_port","TFTP port" to "tftp_port","HTTP port" to "http_port")
        val proxyLabel=tr("ProxyDHCP (external DHCP assigns addresses)")
        dialog(tr("Network"),fields.map{(label,key)->label to settings.optString(key)},listOf("Detect DHCP automatically" to settings.optBoolean("dhcp_auto"),"Allocate addresses" to settings.optBoolean("dhcp_enabled"),"Ethernet protocols" to settings.optBoolean("ethernet_enabled"),proxyLabel to settings.optBoolean("proxy_dhcp_enabled"))){values->
            for((label,key)in fields){val value=values[label].orEmpty();if(key.endsWith("_port")){val port=value.toIntOrNull();if(port==null||port !in 1..65535){Toast.makeText(this,tr("Port must be 1–65535"),Toast.LENGTH_LONG).show();return@dialog};settings.put(key,port)}else settings.put(key,value)}
            val automatic=values["Detect DHCP automatically"].toBoolean()
            settings.put("dhcp_auto",automatic)
            if(!automatic && values["Allocate addresses"].toBoolean() && values[proxyLabel].toBoolean()){Toast.makeText(this,tr("Address allocation and ProxyDHCP cannot both be enabled"),Toast.LENGTH_LONG).show();return@dialog}
            settings.put("proxy_dhcp_enabled",!automatic && values[proxyLabel].toBoolean())
            settings.put("dhcp_enabled",!automatic && values["Allocate addresses"].toBoolean()).put("ethernet_enabled",values["Ethernet protocols"].toBoolean())
            request("/api/settings","PUT",settings){detectNetwork()}
        }
    }
    override fun onResume(){super.onResume();handler.post(poll)}
    override fun onPause(){handler.removeCallbacks(poll);super.onPause()}
    override fun onDestroy(){super.onDestroy();worker.shutdown()}
}
private fun JSONArray.each(action:(JSONObject)->Unit){for(i in 0 until length())action(getJSONObject(i))}
