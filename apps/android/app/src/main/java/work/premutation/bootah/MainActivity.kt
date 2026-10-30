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
    private var page = "设备"
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
    private fun t(zh:String,en:String)=Strings.tr(zh,localeTag).let{if(it==zh&&!chinese)en else it}
    private fun tr(zh:String)=Strings.tr(zh,localeTag)
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
            contentDescription=t("设置","Settings");tooltipText=contentDescription
            scaleType=ImageView.ScaleType.CENTER_INSIDE;setPadding(dp(12),dp(12),dp(12),dp(12))
            setOnClickListener{settings()}
        }
        header.addView(settingsButton,LinearLayout.LayoutParams(dp(48),dp(48)));layout.addView(header)
        val bar=LinearLayout(this).apply{gravity=android.view.Gravity.CENTER_VERTICAL;setPadding(padding,dp(12),padding,dp(12))}
        heading=TextView(this).apply{textSize=30f;setTextColor(ink);setTypeface(null,Typeface.BOLD)};bar.addView(heading,LinearLayout.LayoutParams(0,-2,1f))
        toggle=button(t("启动","Start"),primary=true) {if(snapshot.length()==0){startAfterDetection=true;if(localCore())startForegroundService(Intent(this,CoreService::class.java))}else if(snapshot.optBoolean("running")) request("/api/service/stop","POST",JSONObject()){refresh()} else detectNetwork { request("/api/service/start","POST",JSONObject()){refresh()} }}
        deleteSelectionButton=button(tr("删除")){deleteSelection()}
        bar.addView(deleteSelectionButton);bar.addView(toggle);layout.addView(bar)
        logToolbar=LinearLayout(this).apply{orientation=LinearLayout.VERTICAL;setPadding(padding,0,padding,0)};layout.addView(logToolbar)
        content=LinearLayout(this).apply {orientation=LinearLayout.VERTICAL;setPadding(padding,dp(8),padding,dp(24))}
        contentScroll=ScrollView(this).apply {isFillViewport=true;addView(content)}
        layout.addView(contentScroll,LinearLayout.LayoutParams(-1,0,1f))
        val tabs=LinearLayout(this).apply{setPadding(dp(12),dp(8),dp(12),dp(8));setBackgroundColor(surface)}
        listOf("设备","镜像库","活动").forEach {name->val item=button(tabName(name)){page=name;render()};navigation[name]=item;tabs.addView(item,LinearLayout.LayoutParams(0,dp(64),1f))}
        layout.addView(state,LinearLayout.LayoutParams(-1,dp(24)))
        layout.addView(tabs)
        setContentView(layout);render();refresh()
    }
    private fun button(label:String, primary:Boolean=false, action:()->Unit)=Button(this,null,0,if(primary)android.R.style.Widget_Material_Button_Colored else android.R.style.Widget_Material_Button_Borderless).apply {text=label;isAllCaps=false;setOnClickListener{action()}}
    private fun tabName(name:String)=when(name){"设备"->t("设备","Devices");"镜像库"->t("镜像","Images");else->t("活动","Activity")}
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
                val text=if(code in 200..299&&!ok)tr("核心未确认令牌鉴权") else (if(ok)connection.inputStream else connection.errorStream)?.bufferedReader()?.use{it.readText()} ?: ""
                connection.disconnect()
                runOnUiThread{if(ok)done(text)else{failed(code);failedBody?.invoke(code,text);if(!quiet)Toast.makeText(this,text.lines().joinToString("\n"){message(it)},Toast.LENGTH_LONG).show()}}
            }catch(e:Exception){runOnUiThread{failed(null);state.text=tr("未连接");if(!quiet)Toast.makeText(this,message(e.message.orEmpty()),Toast.LENGTH_LONG).show()}}
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
            if(localCore()&&android.os.Build.VERSION.SDK_INT>=33&&checkSelfPermission(android.Manifest.permission.POST_NOTIFICATIONS)!=android.content.pm.PackageManager.PERMISSION_GRANTED)messages.add(tr("通知权限未开启。请在应用设置中允许 Bootah 发送通知，以显示后台服务状态。"))
            if(messages.isNotEmpty())showPermissionIssues(messages)else if(force)AlertDialog.Builder(this).setTitle(tr("网络权限")).setMessage(tr("无需额外授权")).setPositiveButton(tr("好"),null).show()
        }
    }
    private fun showPermissionIssues(messages:List<String>){
        if(isFinishing||isDestroyed)return
        val dialog=AlertDialog.Builder(this).setTitle(tr("需要处理的权限")).setMessage(messages.joinToString("\n\n")).setPositiveButton(tr("设置")){_,_->settings()}.setNegativeButton(tr("稍后"),null)
        if(localCore()&&android.os.Build.VERSION.SDK_INT>=33&&checkSelfPermission(android.Manifest.permission.POST_NOTIFICATIONS)!=android.content.pm.PackageManager.PERMISSION_GRANTED)dialog.setNeutralButton(tr("前往应用设置")){_,_->startActivity(Intent(android.provider.Settings.ACTION_APP_NOTIFICATION_SETTINGS).putExtra(android.provider.Settings.EXTRA_APP_PACKAGE,packageName))}
        dialog.show()
    }
    private fun refresh(){if(refreshing)return;refreshing=true;request("/api/state",quiet=true,finished={refreshing=false},failed={code->if(code==401)state.text=tr("管理令牌无效或缺失") else if(code!=null && code in 200..299)state.text=tr("核心未确认令牌鉴权") else if(localCore()&&!coreRequested){coreRequested=true;startForegroundService(Intent(this,CoreService::class.java))}}){
        snapshot=JSONObject(it)
        loadAutomationCapabilities()
        val result=snapshot.optJSONObject("network")
        val detection=result?.optString("status").orEmpty()
        val fullStatus=tr(if(snapshot.optBoolean("running"))"运行中" else "已停止")+"  "+tr(detection)
        state.contentDescription=fullStatus
        state.text=tr(if(snapshot.optBoolean("running"))"运行中" else "已停止")+"  "+tr(when(detection){"lease_found"->"DHCP：系统记录";"response_found"->"DHCP：已响应";else->detection})
        toggle.text=tr(if(snapshot.optBoolean("running"))"停止" else "启动");render();checkDeviceConflict()
        if(!snapshot.optBoolean("running")&&!detecting&&networkEvidence().toString()!=networkSignature)detectNetwork()
        else if(!detecting&&!permissionsChecked)checkPermissions()
    }}
    private fun checkDeviceConflict(){
        val conflict=snapshot.optJSONArray("device_conflicts")?.optJSONObject(0)
        if(conflict==null){pendingConflictId=null;return}
        val id=conflict.optString("id");if(id==pendingConflictId)return;pendingConflictId=id
        val details=if(conflict.optString("reason")=="ip-mac")"IP ${conflict.optString("ip")}: ${conflict.optString("old_mac")} → ${conflict.optString("new_mac")}" else "${conflict.optString("new_mac")}: ${conflict.optString("old_architecture")} → ${conflict.optString("new_architecture")}"
        val message=details+"\n"+tr("旧设备有手动配置。清理后，新设备将在下次网络启动时使用默认镜像。")
        val resolve:(Boolean)->Unit={accept->request("/api/device-conflicts/$id","POST",JSONObject().put("accept",accept)){refresh()}}
        AlertDialog.Builder(this).setTitle(tr("清理旧设备记录？")).setMessage(message).setPositiveButton(tr("清理")){_,_->resolve(true)}.setNegativeButton(tr("保留")){_,_->resolve(false)}.setCancelable(false).show()
    }
    private fun deleteSelection(){
        val devices=page=="设备"
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
        deleteSelectionButton.visibility=if(page=="活动")View.GONE else View.VISIBLE
        deleteSelectionButton.isEnabled=!deletingSelection && (if(page=="设备")selectedDeviceId!=null else selectedProfileId!=null)
        logToolbar.removeAllViews();logToolbar.visibility=if(page=="活动")View.VISIBLE else View.GONE
        content.removeAllViews();heading.text=when(page){"设备"->t("设备","Devices");"镜像库"->t("镜像库","Image library");else->t("活动","Activity")}
        for((name,item)in navigation){item.setTextColor(if(name==page)android.graphics.Color.parseColor(if(dark)"#AFCFFF" else "#245EA7") else muted);item.isSelected=name==page}
        when(page){
            "设备"->{actions(t("添加设备","Add device") to {device(JSONObject())});val devices=snapshot.optJSONArray("devices");if(devices==null||devices.length()==0)row(t("暂无设备","No devices yet"),"",""){device(JSONObject())};devices?.each {d->val profiles=snapshot.optJSONArray("profiles");var assigned=t("未分配镜像","No image assigned");val defaultId=snapshot.optJSONObject("default_profiles")?.optString(d.optString("architecture"));profiles?.each{if(it.optString("id")==d.optString("profile_id"))assigned=it.optString("name") else if(d.isNull("profile_id")&&it.optString("id")==defaultId)assigned=tr("按架构自动分配")+"  "+it.optString("name")};row(d.optString("name").ifEmpty{d.optString("mac").ifEmpty{d.optString("ip")}},listOf(d.optString("ip"),d.optString("architecture"),d.optString("model")).filter{it.isNotBlank()&&it!="null"}.joinToString("  "),assigned,selected=d.optString("id")==selectedDeviceId,longAction={deviceMenu(d)}){selectedDeviceId=d.optString("id");render()}}}
            "镜像库"->{architectureDefaults();actions(tr("导入 ISO") to {pickIso()},tr("高级手动配置") to {profile(JSONObject())});isoTrustCheck=CheckBox(this).apply{text=t("信任所选本地 ISO 安装源","Trust selected local ISO source");isChecked=trustLocalIso;setTextColor(ink);setOnCheckedChangeListener{_,checked->trustLocalIso=checked}};content.addView(isoTrustCheck);snapshot.optJSONArray("profiles")?.each {p->row(p.optString("system_name").ifBlank{p.optString("name")},"${p.optString("name")}  ${p.optString("architecture")}  ${p.optString("kind")}",(if(p.optString("status")=="needs-attention")tr("待配置") else p.optString("media")),logoId(p),selected=p.optString("id")==selectedProfileId,longAction={profileMenu(p)}){selectedProfileId=p.optString("id");render()}}}
            else->{val logBar=LinearLayout(this).apply{orientation=LinearLayout.VERTICAL;setPadding(0,dp(4),0,dp(12))};val filterRow=LinearLayout(this).apply{orientation=LinearLayout.HORIZONTAL;gravity=Gravity.CENTER_VERTICAL};filterRow.addView(button(t("日志级别","Log level"),action={chooseLogLevel()}),LinearLayout.LayoutParams(-2,-2).apply{marginEnd=dp(16)});filterRow.addView(CheckBox(this).apply{text=t("包含原始隐私信息","Include private data");isChecked=exportRaw;setTextColor(ink);setOnCheckedChangeListener{_,checked->exportRaw=checked}},LinearLayout.LayoutParams(0,-2,1f));logBar.addView(filterRow);logBar.addView(button(t("导出诊断日志","Export diagnostics"),action={saveDiagnostics()}),LinearLayout.LayoutParams(-2,-2).apply{topMargin=dp(12);gravity=Gravity.END});val logActions=LinearLayout(this).apply{orientation=LinearLayout.HORIZONTAL};logActions.addView(button(tr("清除日志")){request("/api/events","DELETE"){refresh()}},LinearLayout.LayoutParams(0,-2,1f));logActions.addView(button(tr("回到日志顶部")){contentScroll.smoothScrollTo(0,0)},LinearLayout.LayoutParams(0,-2,1f));logBar.addView(logActions);logToolbar.addView(logBar);val events=snapshot.optJSONArray("events");if(events==null||events.length()==0)row(tr("暂无活动"),"",""){};events?.let{for(i in it.length()-1 downTo 0){val e=it.getJSONObject(i);row(e.optString("protocol"),e.optString("message"),e.optString("device")) {}}}}
        }
    }
    private fun chooseLogLevel(){val levels=arrayOf(t("错误","Errors"),t("常规","Standard"),t("详细","Detailed"));val values=arrayOf("error","info","debug");AlertDialog.Builder(this).setTitle(t("日志级别","Log level")).setSingleChoiceItems(levels,values.indexOf(exportLogLevel)){dialog,index->exportLogLevel=values[index];dialog.dismiss()}.show()}
    private fun saveDiagnostics(){
        val start={startActivityForResult(Intent(Intent.ACTION_CREATE_DOCUMENT).apply{addCategory(Intent.CATEGORY_OPENABLE);type="text/plain";putExtra(Intent.EXTRA_TITLE,"bootah-diagnostics.txt")},42)}
        if(exportRaw)AlertDialog.Builder(this).setTitle(t("原始日志包含隐私信息","Raw logs contain private data")).setMessage(t("原始日志可能包含设备地址和本地文件路径。","Raw logs may contain device addresses and local file paths.")).setNegativeButton(tr("取消"),null).setPositiveButton(t("继续","Continue")){_,_->start()}.show() else start()
    }
    private fun exportDiagnostics(uri:android.net.Uri){
        val level=exportLogLevel;val raw=exportRaw
        worker.execute{try{val binary=java.io.File(applicationInfo.nativeLibraryDir,"libnetboot_core.so");val config=java.io.File(filesDir,"state.json");val command=mutableListOf(binary.path,"--export-diagnostics","--config",config.path,"--log-level",level);if(raw)command.add("--include-private");val process=ProcessBuilder(command).start();val data=process.inputStream.use{it.readBytes()};val errors=process.errorStream.bufferedReader().use{it.readText()};if(process.waitFor()!=0)throw java.io.IOException(errors);contentResolver.openOutputStream(uri,"w")?.use{it.write(data)}?:throw java.io.IOException("Cannot open destination");runOnUiThread{Toast.makeText(this,t("诊断日志已导出","Diagnostics exported"),Toast.LENGTH_LONG).show()}}catch(e:Exception){runOnUiThread{Toast.makeText(this,e.message?:t("导出失败","Export failed"),Toast.LENGTH_LONG).show()}}}
    }

    private fun imageDetails(p:JSONObject){
        val panel=LinearLayout(this).apply{orientation=LinearLayout.VERTICAL;setPadding(dp(24),dp(8),dp(24),dp(8))}
        fun field(label:String,value:String){if(value.isBlank()||value=="null")return;panel.addView(TextView(this).apply{text=tr(label);setTextColor(muted);textSize=12f;setPadding(0,dp(10),0,0)});panel.addView(EditText(this).apply{setText(value);setTextColor(ink);textSize=14f;isFocusable=false;isCursorVisible=false;setTextIsSelectable(true);maxLines=5})}
        if(p.optJSONObject("secure_boot")?.optBoolean("eligible")==true)request("/api/profiles/${p.getString("id")}/secure-boot",quiet=true){text->p.put("secure_boot",JSONObject(text))}
        if(p.optJSONObject("secure_boot")?.optBoolean("eligible")==true)panel.addView(Button(this).apply{text=t("安全启动支持","Secure Boot support");setOnClickListener{secureBootSettings(p.getString("id"))}})
        field("名称",p.optString("name"));field("架构",p.optString("architecture"));field("类型",p.optString("kind"));field("镜像文件",p.optString("media"));field("目录",p.optString("root"))
        p.optJSONObject("boot_evidence")?.let{e->field("入口",e.optString("entry"));field("启动配置",e.optString("config"));field("内核",e.optString("kernel"));field("Initrd",e.optString("initrd"));field("安装源",tr(when(e.optString("source")){"http-tree"->"HTTP 安装树";"http-iso"->"HTTP 原盘";"nfs-tree"->"只读 NFS 文件树";"nbd"->"只读 NBD 原盘";"external"->"外部安装源";"local-media"->"需要本地介质";else->"未识别安装源"}));e.optJSONArray("diagnostics")?.each{d->if(d.optString("code") !in setOf("BOOT_CONFIG_FAMILY_LAYOUT","UOS_MANUAL_ONLY"))field(if(d.optString("code") in setOf("SOURCE_REQUIRED","SOURCE_UNVERIFIED"))"需要配置安装源" else "需要处理的问题",message(d.optString("detail")))}}
        field("参数",p.optString("arguments"));p.optJSONObject("boot_files")?.let{files->field("启动文件映射",files.keys().asSequence().sorted().joinToString("\n"){"$it=${files.getString(it)}"})}
        AlertDialog.Builder(this).setTitle(tr("镜像详情")).setView(ScrollView(this).apply{addView(panel)}).setPositiveButton(tr("关闭"),null).show()
    }
    private fun secureBootSettings(id:String){
        request("/api/profiles/$id/secure-boot"){text->
            val support=JSONObject(text)
            if(!support.optBoolean("eligible"))return@request
            val panel=LinearLayout(this).apply{orientation=LinearLayout.VERTICAL;setPadding(dp(24),dp(12),dp(24),dp(12))}
            val toggle=Switch(this).apply{this.text=t("安全启动支持","Secure Boot support");isChecked=support.optBoolean("enabled");isEnabled=isChecked||support.optString("state")=="passed"};panel.addView(toggle)
            panel.addView(TextView(this).apply{this.text=message(support.optString("detail")).ifBlank{t("尚未检查","Not checked")};setTextIsSelectable(true)})
            val results=Button(this).apply{this.text=t("查看检查结果","View check results");setOnClickListener{val items=support.optJSONArray("components")?:JSONArray();val content=(0 until items.length()).joinToString("\n\n"){i->val c=items.getJSONObject(i);"${c.optString("file")}\n${c.optString("signer")}\n${c.optString("issuer")}\nSHA-256: ${c.optString("sha256")}\n${c.optString("certificate_sha256")}"};AlertDialog.Builder(this@MainActivity).setTitle(t("查看检查结果","View check results")).setMessage(content.ifBlank{message(support.optString("detail"))}).setPositiveButton(tr("关闭"),null).show()}};panel.addView(results)
            val dialog=AlertDialog.Builder(this).setTitle(t("启动设置","Boot settings")).setView(ScrollView(this).apply{addView(panel)}).setNegativeButton(tr("关闭"),null).create()
            toggle.setOnCheckedChangeListener{_,enabled->toggle.isEnabled=false;request("/api/profiles/$id/secure-boot","PUT",JSONObject().put("enabled",enabled),failed={dialog.dismiss();secureBootSettings(id)}){dialog.dismiss();refresh();secureBootSettings(id)}}

            dialog.show()
        }
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
    private fun deviceMenu(d:JSONObject){itemMenu(d.optString("name").ifBlank{d.optString("mac")},arrayOf(tr("编辑"),tr("自动安装配置")),booleanArrayOf(true,automationSupported(deviceProfile(d)))){which->if(which==0)device(d) else chooseAutomation("device",d.optString("id"))}}

    private fun pickIso(){pendingIsoTrust=trustLocalIso;trustLocalIso=false;isoTrustCheck?.isChecked=false;startActivityForResult(Intent(Intent.ACTION_OPEN_DOCUMENT).apply{type="*/*";addCategory(Intent.CATEGORY_OPENABLE);putExtra(Intent.EXTRA_MIME_TYPES,arrayOf("application/x-iso9660-image","application/octet-stream"))},40)}
    @Deprecated("Framework activity result compatibility")
    override fun onActivityResult(requestCode:Int,resultCode:Int,data:Intent?){super.onActivityResult(requestCode,resultCode,data);if(requestCode==41){val pending=pendingAutomation;val next=pendingAutomationNext;pendingAutomation=null;pendingAutomationNext=null;if(resultCode==RESULT_OK&&pending!=null&&data?.data!=null)uploadAutomation(pending,data.data!!,next?:{} ) else next?.invoke();return};if(resultCode!=RESULT_OK)return;val uri=data?.data?:return
        if(requestCode==42){exportDiagnostics(uri);return}
        if(requestCode!=40)return
        if(endpoint!="http://127.0.0.1:9080"){Toast.makeText(this,tr("ISO 导入需连接本机服务"),Toast.LENGTH_LONG).show();return}
        importUri(uri,pendingIsoTrust)
    }
    private fun importUri(uri:android.net.Uri,trustSelectedIso:Boolean=false){
        state.text=t("正在导入…","Importing…")
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
                        else Toast.makeText(this,t("未能确认导入结果，已保留 ISO 以免破坏启动引用。","Import outcome is uncertain; ISO retained to preserve boot references."),Toast.LENGTH_LONG).show()
                    }){text->
                        // Profiles read this original ISO on demand after import.
                        val result=JSONObject(text);refresh()
                        val warnings=result.optJSONArray("warnings")
                        val notes=(0 until (warnings?.length()?:0)).map{warnings!!.getString(it)}
                        Toast.makeText(this,tr("ISO 导入完成"),Toast.LENGTH_SHORT).show()
                        if(notes.isNotEmpty())AlertDialog.Builder(this).setTitle(tr("需要处理的问题")).setMessage(notes.map(::message).joinToString("\n\n")).setPositiveButton(tr("确定"),null).show()
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
        request("/api/profiles/$profileId/automation-methods"){response->val array=JSONArray(response);val methods=(0 until array.length()).map{array.getString(it)};if(methods.isEmpty()){after();return@request};val choices=listOf(t("不使用自动配置","No automatic configuration"))+methods;AlertDialog.Builder(this).setTitle(t("自动安装配置","Automatic installation")).setItems(choices.toTypedArray()){_,index->if(index==0){request("/api/automation/$scope/$id","DELETE"){refresh();after()}}else{pendingAutomation=Triple(scope,id,methods[index-1]);pendingAutomationNext=after;startActivityForResult(Intent(Intent.ACTION_OPEN_DOCUMENT).apply{type="*/*";addCategory(Intent.CATEGORY_OPENABLE)},41)}}.setNegativeButton(t("稍后","Later")){_,_->after()}.show()}
    }
    private fun uploadAutomation(pending:Triple<String,String,String>,uri:android.net.Uri,next:()->Unit={}){worker.execute{try{val name=contentResolver.query(uri,arrayOf(android.provider.OpenableColumns.DISPLAY_NAME),null,null,null)?.use{if(it.moveToFirst())it.getString(0) else "config"}?:"config";val address="$endpoint/api/automation/${pending.first}/${pending.second}?method=${java.net.URLEncoder.encode(pending.third,"UTF-8")}&filename=${java.net.URLEncoder.encode(name,"UTF-8")}";val connection=URL(address).openConnection() as HttpURLConnection;authorize(connection);connection.requestMethod="PUT";connection.doOutput=true;connection.connectTimeout=5000;connection.readTimeout=30000;contentResolver.openInputStream(uri)!!.use{input->connection.outputStream.use{input.copyTo(it)}};val code=connection.responseCode;val error=if(code in 200..299 && connection.getHeaderField("X-Bootah-Authentication")!="bearer-v1")tr("核心未确认令牌鉴权") else if(code !in 200..299)connection.errorStream?.bufferedReader()?.use{it.readText()}?.let{tr(it)} else null;connection.disconnect();runOnUiThread{if(error!=null)Toast.makeText(this,error,Toast.LENGTH_LONG).show();refresh();next()}}catch(e:Exception){runOnUiThread{Toast.makeText(this,e.message?.let{tr(it)}?:t("请求失败","Request failed"),Toast.LENGTH_LONG).show();next()}}}}
    private fun dialog(title:String,fields:List<Pair<String,String>>,checks:List<Pair<String,Boolean>> = emptyList(),save:(Map<String,String>)->Unit){
        val panel=LinearLayout(this).apply{orientation=LinearLayout.VERTICAL;setPadding(32,12,32,12)}
        val inputs=fields.map{(label,value)->val input=EditText(this).apply{hint=tr(label);setText(value);isSingleLine=label!="启动文件映射"};panel.addView(input);label to input}.toMap()
        val toggles=checks.map{(label,value)->val input=CheckBox(this).apply{text=tr(label);isChecked=value};panel.addView(input);label to input}.toMap()
        AlertDialog.Builder(this).setTitle(title).setView(ScrollView(this).apply{addView(panel)}).setNegativeButton(tr("取消"),null).setPositiveButton(tr("保存")){_,_->save(inputs.mapValues{it.value.text.toString()}+toggles.mapValues{it.value.isChecked.toString()})}.show()
    }
    private fun device(d:JSONObject){
        val panel=LinearLayout(this).apply{orientation=LinearLayout.VERTICAL;setPadding(dp(24),dp(8),dp(24),dp(8))}
        panel.addView(TextView(this).apply{text=tr("至少填写 MAC 或 IP");setTextColor(muted);textSize=13f})
        val inputs=listOf("MAC" to "mac","IP" to "ip","名称" to "name","型号" to "model").associate{(label,key)->panel.addView(TextView(this).apply{text=tr(label);textSize=12f;setTextColor(muted);setPadding(0,dp(12),0,0)});val edit=EditText(this).apply{isSingleLine=true;setText(d.optString(key).takeIf{it!="null"}.orEmpty())};panel.addView(edit);key to edit}
        val profiles=snapshot.optJSONArray("profiles")?:JSONArray();val defaultId=snapshot.optJSONObject("default_profiles")?.optString(d.optString("architecture"));val defaultName=(0 until profiles.length()).map{profiles.getJSONObject(it)}.firstOrNull{it.optString("id")==defaultId}?.optString("name");val names=mutableListOf(defaultName?.let{tr("按架构自动分配")+"  "+it}?:tr("未分配镜像"));profiles.each{names.add(it.optString("name"))};panel.addView(TextView(this).apply{text=tr("启动镜像");setTextColor(muted);textSize=12f;setPadding(0,dp(16),0,0)})
        val picker=Spinner(this);picker.adapter=ArrayAdapter(this,android.R.layout.simple_spinner_dropdown_item,names);for(i in 0 until profiles.length())if(profiles.getJSONObject(i).optString("id")==d.optString("profile_id"))picker.setSelection(i+1);panel.addView(picker)
        AlertDialog.Builder(this).setTitle(tr("设备")).setView(ScrollView(this).apply{addView(panel)}).setNegativeButton(tr("取消"),null).setPositiveButton(tr("保存")){_,_->if(inputs["mac"]?.text.isNullOrBlank()&&inputs["ip"]?.text.isNullOrBlank()){Toast.makeText(this,tr("至少填写 MAC 或 IP"),Toast.LENGTH_LONG).show();return@setPositiveButton};for((key,edit)in inputs)d.put(key,if(key=="ip"&&edit.text.isBlank())JSONObject.NULL else edit.text.toString());d.put("profile_id",if(picker.selectedItemPosition==0)JSONObject.NULL else profiles.getJSONObject(picker.selectedItemPosition-1).getString("id"));request("/api/devices","POST",d){refresh()}}.show()
    }
    private fun profile(p:JSONObject){
        val fields=listOf("名称" to p.optString("name"),"架构" to p.optString("architecture","arm64"),"类型" to p.optString("kind","linux"),"目录" to p.optString("root"),"内核" to p.optString("kernel"),"Initrd" to p.optString("initrd"),"DTB" to p.optString("dtb"),"参数" to p.optString("arguments"),"启动文件映射" to p.optJSONObject("boot_files")?.let{f->f.keys().asSequence().joinToString("\n"){"$it=${f.getString(it)}"}}.orEmpty())
        dialog(tr("高级手动配置"),fields){v->val files=JSONObject();for(line in v["启动文件映射"].orEmpty().lines().filter{it.isNotBlank()}){val pair=line.split('=',limit=2);if(pair.size!=2){Toast.makeText(this,tr("请按 x64-efi=bootx64.efi 的格式填写，每行一项"),Toast.LENGTH_LONG).show();return@dialog};files.put(pair[0].trim(),pair[1].trim())};p.put("id",p.optString("id",java.util.UUID.randomUUID().toString())).put("name",v["名称"]).put("architecture",v["架构"]).put("kind",v["类型"]).put("root",v["目录"]).put("kernel",v["内核"]).put("initrd",v["Initrd"]).put("dtb",v["DTB"]).put("arguments",v["参数"]).put("boot_files",files);request("/api/profiles","POST",p){refresh()}}
    }
    private fun profileMenu(p:JSONObject){
        val labels=arrayOf(tr("编辑"),tr("自动安装配置"),tr("镜像详情"),tr("启动方式"),tr("删除"))
        itemMenu(p.optString("name"),labels,booleanArrayOf(true,automationSupported(p.optString("id")),true,true,true)){index->when(index){0->profile(p);1->chooseAutomation("profile",p.getString("id"));2->imageDetails(p);3->manual(p);4->request("/api/profiles/${p.getString("id")}","DELETE"){refresh()}}}
    }
    private val firmwareKeys=setOf("x86-bios","ia64","ia32-efi","x64-efi","ebc-efi","alpha","powerpc-of","powerpc-epapr","power-opal","arm32-efi","arm64-efi","ia32-http","x64-http","arm32-http","arm64-http","arm32-uboot","arm64-uboot","riscv32-efi","riscv64-http","loongarch64-efi","riscv64-efi","hp-rmp")
    private fun architectureDefaults(){
        content.addView(TextView(this).apply{text=tr("架构默认启动镜像");setTextColor(ink);textSize=18f})
        val defaults=snapshot.optJSONObject("default_profiles")?:JSONObject()
        val profiles=snapshot.optJSONArray("profiles")?:JSONArray()
        for(key in (0 until profiles.length()).flatMap{profiles.getJSONObject(it).optJSONObject("boot_files")?.keys()?.asSequence()?.toList().orEmpty()}.filter{it in firmwareKeys}.distinct().sorted()){
            val candidates=(0 until profiles.length()).map{profiles.getJSONObject(it)}.filter{it.optJSONObject("boot_files")?.has(key)==true}
            val name=candidates.firstOrNull{it.optString("id")==defaults.optString(key)}?.optString("name")?:tr("未设置")
            row(key,"",name){
                val labels=(listOf(tr("未设置"))+candidates.map{it.optString("name")}).toTypedArray()
                val selected=candidates.indexOfFirst{it.optString("id")==defaults.optString(key)}+1
                AlertDialog.Builder(this).setTitle(key).setSingleChoiceItems(labels,selected){dialog,index->
                    val changed=JSONObject((snapshot.optJSONObject("default_profiles")?:JSONObject()).toString())
                    if(index==0)changed.remove(key) else changed.put(key,candidates[index-1].getString("id"))
                    dialog.dismiss();request("/api/default-profiles","PUT",changed){refresh()}
                }.setNegativeButton(tr("取消"),null).show()
            }
        }
    }
    private fun manual(p:JSONObject){
        request("/api/profiles/${p.getString("id")}/boot-commands?lang=${if(chinese)"zh" else "en"}"){response->
            val result=JSONObject(response)
            val commands=result.optString("commands")
            val notesArray=result.optJSONArray("notes")
            val notes=(0 until (notesArray?.length()?:0)).mapNotNull{notesArray?.optString(it)}.map(::message).joinToString("\n")
            val status=tr(if(commands.isNotBlank())"在固件中执行以下命令" else if(result.optString("firmware") in setOf("pxe","efi"))"从固件菜单选择网络启动，无需输入命令" else if(result.optString("firmware")=="openfirmware")"请使用固件的网络启动入口" else "请先完成启动配置，详见下方说明")
            val panel=LinearLayout(this).apply{orientation=LinearLayout.VERTICAL;setPadding(dp(24),dp(8),dp(24),dp(8))}
            panel.addView(TextView(this).apply{text=status;textSize=13f;setTextColor(muted)})
            if(commands.isNotBlank())panel.addView(TextView(this).apply{text=commands;typeface=Typeface.MONOSPACE;textSize=13f;setTextColor(ink);setTextIsSelectable(true);setPadding(0,dp(12),0,0)})
            if(notes.isNotBlank())panel.addView(TextView(this).apply{text=notes;textSize=13f;setTextColor(muted);setPadding(0,dp(12),0,0)})
            val dialog=AlertDialog.Builder(this).setTitle(result.optString("title")).setView(ScrollView(this).apply{addView(panel)}).setNegativeButton(t("关闭","Close"),null)
            if(commands.isNotBlank())dialog.setPositiveButton(t("复制命令","Copy commands")){_,_->(getSystemService(Context.CLIPBOARD_SERVICE) as android.content.ClipboardManager).setPrimaryClip(android.content.ClipData.newPlainText("Boot commands",commands))}
            dialog.show()
        }
    }
    private fun settings(){
        AlertDialog.Builder(this).setTitle(tr("设置")).setItems(arrayOf("连接","网络","本机启动","Root 启动","结束本机服务","关于与开源许可","重新检查权限").map(::tr).plus(t("Windows 安装源服务","Windows installation source")).toTypedArray()){_,index->
            if(index==0)connectionSettings()
            else when(index){1->network();2->startForegroundService(Intent(this,CoreService::class.java));3->startForegroundService(Intent(this,CoreService::class.java).putExtra("root",true));4->stopService(Intent(this,CoreService::class.java));5->about();6->checkPermissions(true);7->configureSmb()}
        }.show()
    }
    private fun connectionSettings(){
        val panel=LinearLayout(this).apply{orientation=LinearLayout.VERTICAL;setPadding(dp(24),dp(12),dp(24),0)}
        val address=EditText(this).apply{hint=tr("服务地址");setText(endpoint);isSingleLine=true}
        val secret=EditText(this).apply{hint=tr("管理令牌");inputType=android.text.InputType.TYPE_CLASS_TEXT or android.text.InputType.TYPE_TEXT_VARIATION_PASSWORD;setText(if(localCore()&&ApiCredentials.localToken!=null)"" else token().orEmpty())}
        val save=CheckBox(this).apply{text=tr("安全保存令牌");isChecked=true}
        panel.addView(address);panel.addView(secret);panel.addView(save)
        AlertDialog.Builder(this).setTitle(tr("连接")).setView(panel).setNegativeButton(tr("取消"),null).setPositiveButton(tr("保存")){_,_->
            val url=address.text.toString().trim().trimEnd('/');val value=secret.text.toString().trim();getPreferences(0).edit().putString("endpoint",url).apply();sessionToken=value.ifEmpty{null}
            if(save.isChecked&&value.isNotEmpty())try{ApiCredentials.save(this,url,value)}catch(_:Exception){ApiCredentials.remove(this,url);Toast.makeText(this,tr("无法安全保存令牌"),Toast.LENGTH_LONG).show()} else ApiCredentials.remove(this,url)
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
            val options=arrayOf(t("关闭","Off"),t("自动（按镜像）","Automatic (by image)"),"SMB1 — XP / Server 2003","SMB2 — Vista / Server 2008+")
            AlertDialog.Builder(this).setTitle(t("Windows 安装源服务","Windows installation source")+" — "+state.optString("mode"))
                .setSingleChoiceItems(options,selected){_,index->selected=index}
                .setPositiveButton(t("应用","Apply")){_,_->switchSmb(modes[selected])}
                .setNegativeButton(tr("取消")){_,_->changingSmb=false}
                .setOnCancelListener{changingSmb=false}.show()
        }
    }
    private fun switchSmb(mode:String,expected:String?=null){
        val body=JSONObject().put("mode",mode).put("confirm",expected!=null)
        if(expected!=null)body.put("expected_mode",expected)
        request("/api/smb","POST",body,quiet=true,failed={code->
            if(code==null){changingSmb=false;Toast.makeText(this,t("无法连接安装源服务，请重试。","Could not reach the installation source service. Retry."),Toast.LENGTH_LONG).show()}
        },failedBody={code,text->
            val conflict=try{JSONObject(text)}catch(_:Exception){JSONObject()}
            if(code==409&&conflict.optString("code")=="smb_switch_confirmation"){
                AlertDialog.Builder(this).setTitle(t("停止当前安装源服务？","Stop the current installation source?"))
                    .setMessage(conflict.optString(if(chinese)"message_zh" else "message")+"\n"+t("正在进行的 Windows 安装可能中断。","Windows installations in progress may be interrupted."))
                    .setPositiveButton(if(mode=="off")t("停止服务","Stop service") else t("停止并切换","Stop and switch")){_,_->switchSmb(mode,conflict.getString("current_mode"))}
                    .setNegativeButton(tr("取消")){_,_->changingSmb=false}
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
                    addView(labeledLink("来源",entry.getString("source")){open(entry.getString("source"))})
                    addView(labeledLink("许可协议",tr(entry.getString("license"))){
                        val notice=entry.getString("notice")
                        if(notice.isEmpty())open(entry.getString("licenseUrl")) else {
                            val text=TextView(this@MainActivity).apply{setText(notice);textSize=13f;setTextColor(ink);setTextIsSelectable(true);setPadding(dp(24),dp(16),dp(24),dp(16))}
                            AlertDialog.Builder(this@MainActivity).setTitle(entry.getString("name")+" — "+tr("许可协议")).setView(ScrollView(this@MainActivity).apply{addView(text)}).setPositiveButton(tr("关闭"),null).show()
                        }
                    })
                }
            }
        }
        val header=LinearLayout(this).apply{orientation=LinearLayout.VERTICAL;setPadding(dp(24),dp(12),dp(24),dp(12))
            addView(TextView(this@MainActivity).apply{text="${tr("客户端版本")}: ${BuildConfig.VERSION_NAME} (${BuildConfig.VERSION_CODE})";setTextColor(ink)})
            addView(TextView(this@MainActivity).apply{text="${tr("核心版本")}: "+(coreVersion?.let{"${it.optString("display")} (${it.optInt("build")})"}?:tr("不可用"));setTextColor(ink)})}
        AlertDialog.Builder(this).setTitle(tr("关于与开源许可")).setView(ListView(this).apply{addHeaderView(header,null,false);setAdapter(adapter);divider=null}).setPositiveButton(tr("关闭"),null).show()
    }
    private fun network(){
        val settings=snapshot.optJSONObject("settings")?:return
        val fields=linkedMapOf("服务器 IPv4" to "server_ip","监听 IPv4" to "bind_ip","地址池起点" to "pool_start","地址池终点" to "pool_end","子网掩码" to "subnet","DHCP 端口" to "dhcp_port","TFTP 端口" to "tftp_port","HTTP 端口" to "http_port")
        val proxyLabel=t("ProxyDHCP（外部 DHCP 分配地址）","ProxyDHCP (external DHCP assigns addresses)")
        dialog(tr("网络"),fields.map{(label,key)->label to settings.optString(key)},listOf("自动检测 DHCP" to settings.optBoolean("dhcp_auto"),"分配地址" to settings.optBoolean("dhcp_enabled"),"以太网协议" to settings.optBoolean("ethernet_enabled"),proxyLabel to settings.optBoolean("proxy_dhcp_enabled"))){values->
            for((label,key)in fields){val value=values[label].orEmpty();if(key.endsWith("_port")){val port=value.toIntOrNull();if(port==null||port !in 1..65535){Toast.makeText(this,tr("端口应为 1–65535"),Toast.LENGTH_LONG).show();return@dialog};settings.put(key,port)}else settings.put(key,value)}
            val automatic=values["自动检测 DHCP"].toBoolean()
            settings.put("dhcp_auto",automatic)
            if(!automatic && values["分配地址"].toBoolean() && values[proxyLabel].toBoolean()){Toast.makeText(this,t("分配地址与 ProxyDHCP 不能同时开启","Address allocation and ProxyDHCP cannot both be enabled"),Toast.LENGTH_LONG).show();return@dialog}
            settings.put("proxy_dhcp_enabled",!automatic && values[proxyLabel].toBoolean())
            settings.put("dhcp_enabled",!automatic && values["分配地址"].toBoolean()).put("ethernet_enabled",values["以太网协议"].toBoolean())
            request("/api/settings","PUT",settings){detectNetwork()}
        }
    }
    override fun onResume(){super.onResume();handler.post(poll)}
    override fun onPause(){handler.removeCallbacks(poll);super.onPause()}
    override fun onDestroy(){super.onDestroy();worker.shutdown()}
}
private fun JSONArray.each(action:(JSONObject)->Unit){for(i in 0 until length())action(getJSONObject(i))}
