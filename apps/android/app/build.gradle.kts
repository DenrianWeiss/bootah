import groovy.json.JsonSlurper

plugins { id("com.android.application"); id("org.jetbrains.kotlin.android") }
val versionsFile = rootProject.file("../../versions.json")
val versions = JsonSlurper().parse(versionsFile) as Map<*, *>
val androidVersion = versions["android"] as Map<*, *>
val displayVersion = androidVersion["display"] as String
val internalVersion = (androidVersion["build"] as Number).toInt()
require(displayVersion.matches(Regex("(0|[1-9][0-9]*)\\.(0|[1-9][0-9]*)\\.(0|[1-9][0-9]*)"))) { "Invalid Android display version" }
require(internalVersion in 1..65534 && displayVersion.split('.').all { it.toIntOrNull()?.let { n -> n <= 65534 } == true }) { "Invalid Android version component or build" }
android {
    namespace = "work.premutation.bootah"
    compileSdk = 35
    defaultConfig { applicationId = "work.premutation.bootah"; minSdk = 26; targetSdk = 35; versionCode = internalVersion; versionName = displayVersion }
    compileOptions { sourceCompatibility = JavaVersion.VERSION_17; targetCompatibility = JavaVersion.VERSION_17 }
    kotlinOptions { jvmTarget = "17" }
    buildFeatures { buildConfig = true }
    packaging { jniLibs { useLegacyPackaging = true } }
    sourceSets.getByName("main").java.srcDir(layout.buildDirectory.dir("generated/source/i18n"))
    sourceSets.getByName("main").assets.srcDir(layout.buildDirectory.dir("generated/assets"))
}
val copySystemIcons by tasks.registering(Copy::class) {
    from(rootProject.file("../../resources/system-icons")) { include("**/*.png") }
    into(layout.buildDirectory.dir("generated/assets/system-icons"))
}
val generateI18n by tasks.registering(Exec::class) {
    val dictionary = rootProject.file("../../i18n/terms.json")
    val generator = rootProject.file("../../scripts/generate_i18n.py")
    val output = layout.buildDirectory.file("generated/source/i18n/Strings.kt")
    inputs.files(dictionary, generator)
    outputs.file(output)
    commandLine("python3", generator.absolutePath, "--platform", "android", "--output", output.get().asFile.absolutePath)
}
tasks.named("preBuild") { dependsOn(generateI18n, copySystemIcons) }

val copyThirdPartyNotices by tasks.registering(Copy::class) {
    from(rootProject.file("../../LICENSES/third-party.json"))
    into(layout.buildDirectory.dir("generated/assets"))
}
tasks.named("preBuild") { dependsOn(copyThirdPartyNotices) }
