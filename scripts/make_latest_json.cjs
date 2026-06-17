const fs = require("fs");
const path = require("path");

function walk(dir, out = []) {
  if (!fs.existsSync(dir)) return out;
  for (const entry of fs.readdirSync(dir, { withFileTypes: true })) {
    const full = path.join(dir, entry.name);
    if (entry.isDirectory()) walk(full, out);
    else out.push(full);
  }
  return out;
}

function platformFor(filePath) {
  const lower = filePath.toLowerCase();
  if (lower.endsWith(".exe")) {
    if (lower.includes("aarch64") || lower.includes("arm64")) return "windows-aarch64";
    return "windows-x86_64";
  }
  if (lower.endsWith(".appimage")) {
    if (lower.includes("aarch64") || lower.includes("arm64")) return "linux-aarch64";
    return "linux-x86_64";
  }
  // macOS updater artifact is the .app.tar.gz (the .dmg is never signed).
  if (lower.endsWith(".app.tar.gz")) {
    if (lower.includes("aarch64") || lower.includes("arm64") || lower.includes("apple-silicon")) return "darwin-aarch64";
    return "darwin-x86_64";
  }
  if (lower.endsWith(".dmg")) {
    if (lower.includes("aarch64") || lower.includes("arm64") || lower.includes("apple-silicon")) return "darwin-aarch64";
    return "darwin-x86_64";
  }
  return null;
}

function githubReleaseAssetName(fileName) {
  // softprops/action-gh-release normalizes spaces in uploaded asset names to dots.
  // Keep the local file/signature names untouched, but generate URLs matching the
  // actual GitHub Release asset names so the Tauri updater does not hit 404.
  return fileName.replace(/\s+/g, ".");
}

function main() {
  const root = path.resolve(__dirname, "..");
  const pkg = JSON.parse(fs.readFileSync(path.join(root, "package.json"), "utf8"));
  const version = pkg.version;
  const repository = process.env.GITHUB_REPOSITORY || "PeterGriffin888/beeapi-switch";
  const tag =
    process.env.GITHUB_REF_NAME && process.env.GITHUB_REF_NAME.startsWith("v")
      ? process.env.GITHUB_REF_NAME
      : `v${version}`;

  const searchRoots = [
    path.join(root, "src-tauri", "target"),
    path.join(root, "release", "artifacts"),
  ];
  const releaseDir = path.join(root, "release", "update");
  fs.mkdirSync(releaseDir, { recursive: true });

  const allFiles = searchRoots.flatMap((dir) => walk(dir));
  const sigFiles = allFiles.filter((file) => file.endsWith(".sig"));
  const platforms = {};
  const copied = new Set();

  for (const sigPath of sigFiles) {
    const assetPath = sigPath.slice(0, -4);
    if (!fs.existsSync(assetPath)) continue;
    const assetName = path.basename(assetPath);
    const platform = platformFor(assetPath);
    if (!platform) continue;

    const signature = fs.readFileSync(sigPath, "utf8").trim();
    const encodedAssetName = encodeURIComponent(githubReleaseAssetName(assetName)).replace(/'/g, "%27");
    platforms[platform] = {
      signature,
      url: `https://github.com/${repository}/releases/download/${tag}/${encodedAssetName}`,
    };

    if (!copied.has(assetName)) {
      fs.copyFileSync(assetPath, path.join(releaseDir, assetName));
      copied.add(assetName);
    }
    const sigName = `${assetName}.sig`;
    if (!copied.has(sigName)) {
      fs.copyFileSync(sigPath, path.join(releaseDir, sigName));
      copied.add(sigName);
    }
  }

  // macOS .dmg files carry no updater signature, so the loop above never copies
  // them. Publish them anyway so users can download and install manually.
  for (const file of allFiles) {
    if (!file.toLowerCase().endsWith(".dmg")) continue;
    const assetName = path.basename(file);
    if (copied.has(assetName)) continue;
    fs.copyFileSync(file, path.join(releaseDir, assetName));
    copied.add(assetName);
  }

  if (Object.keys(platforms).length === 0) {
    throw new Error(
      "No updater signatures found. Make sure createUpdaterArtifacts is enabled and signing secrets are configured.",
    );
  }

  const latest = {
    version,
    notes: "版本号升级到 0.1.4；软件启动时自动检查 GitHub Release 更新，有新版本时自动下载安装；发布流程会生成 Tauri 自动更新所需的 latest.json。",
    pub_date: new Date().toISOString(),
    platforms,
  };

  fs.writeFileSync(path.join(releaseDir, "latest.json"), `${JSON.stringify(latest, null, 2)}\n`, "utf8");
  console.log(`Generated ${path.join(releaseDir, "latest.json")}`);
  console.log(JSON.stringify(latest, null, 2));
}

module.exports = { walk, platformFor, githubReleaseAssetName, main };

if (require.main === module) {
  main();
}
