import PackagePlugin
import Foundation

@main struct GenerateI18n: BuildToolPlugin {
    func createBuildCommands(context: PluginContext, target: Target) throws -> [Command] {
        let root = context.package.directory.appending("../..")
        let script = root.appending("scripts/generate_i18n.py")
        let dictionary = root.appending("i18n/terms.json")
        let output = context.pluginWorkDirectory.appending("Strings.swift")
        let noticeScript = root.appending("scripts/generate_notices.py")
        let inventory = root.appending("LICENSES/third-party.json")
        let notices = context.pluginWorkDirectory.appending("ThirdParty.swift")
        let versionScript = root.appending("scripts/generate_versions.py")
        let versionSource = root.appending("versions.json")
        let versionOutput = context.pluginWorkDirectory.appending("Version.swift")
        return [.buildCommand(displayName:"Generate version metadata",executable:Path("/usr/bin/env"),arguments:["python3",versionScript.string,"--platform","macos","--output",versionOutput.string],inputFiles:[versionScript,versionSource],outputFiles:[versionOutput]),.buildCommand(displayName:"Generate component notices",executable:Path("/usr/bin/env"),arguments:["python3",noticeScript.string,"--output",notices.string],inputFiles:[noticeScript,inventory],outputFiles:[notices]),.buildCommand(displayName:"Generate Bootah localizations",executable:Path("/usr/bin/env"),arguments:["python3",script.string,"--platform","macos","--output",output.string],inputFiles:[script,dictionary],outputFiles:[output])]
    }
}
