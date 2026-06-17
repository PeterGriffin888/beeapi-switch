import { check, type DownloadEvent } from "@tauri-apps/plugin-updater";

let updateStarted = false;

export async function checkForAppUpdate() {
  if (updateStarted) return;
  updateStarted = true;

  try {
    const update = await check({ timeout: 15_000 });
    if (!update) return;

    let downloaded = 0;
    await update.downloadAndInstall((event: DownloadEvent) => {
      if (event.event === "Started") {
        downloaded = 0;
        console.info(
          `发现新版本 ${update.version}，开始自动更新`,
          event.data.contentLength ? `总大小 ${event.data.contentLength} 字节` : "",
        );
      } else if (event.event === "Progress") {
        downloaded += event.data.chunkLength;
      } else if (event.event === "Finished") {
        console.info(`新版本 ${update.version} 下载完成，已安装更新`, {
          currentVersion: update.currentVersion,
          downloaded,
        });
      }
    });
  } catch (error) {
    console.info("自动更新检查失败，跳过本次更新", error);
  }
}
