// swift-tools-version: 6.0
import PackageDescription
let package = Package(name: "Netboot", platforms: [.macOS(.v14)], products: [.executable(name:"Netboot", targets:["Netboot"])], targets: [.executableTarget(name:"Netboot",plugins:[.plugin(name:"GenerateI18n")]),.plugin(name:"GenerateI18n",capability:.buildTool())])
