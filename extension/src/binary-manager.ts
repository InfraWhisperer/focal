import * as vscode from "vscode";
import * as fs from "fs";
import * as path from "path";
import * as os from "os";
import * as https from "https";
import * as http from "http";

const GITHUB_API_RELEASE =
  "https://api.github.com/repos/InfraWhisperer/focal/releases/latest";

const RELEASES_URL =
  "https://github.com/InfraWhisperer/focal/releases/latest";

interface GitHubAsset {
  name: string;
  browser_download_url: string;
}

interface GitHubRelease {
  assets: GitHubAsset[];
}

/**
 * Maps platform/arch to the GitHub release asset name.
 * Returns undefined for unsupported combinations.
 */
function binaryAssetName(): string | undefined {
  const platform = os.platform();
  const arch = os.arch();

  const map: Record<string, Record<string, string>> = {
    darwin: {
      arm64: "focal-darwin-arm64",
      x64: "focal-darwin-amd64",
    },
    linux: {
      x64: "focal-linux-amd64",
      arm64: "focal-linux-arm64",
    },
    win32: {
      x64: "focal-windows-amd64.exe",
    },
  };

  return map[platform]?.[arch];
}

/**
 * Returns the cached binary path: ~/.focal/bin/focal (or focal.exe on Windows).
 */
function cachedBinaryPath(): string {
  const name = os.platform() === "win32" ? "focal.exe" : "focal";
  return path.join(os.homedir(), ".focal", "bin", name);
}

/**
 * Resolves the focal binary with auto-download fallback.
 *
 * Resolution order:
 *   1. User-configured path (focal.coreBinaryPath)
 *   2. Bundled binary in extension/bin/
 *   3. Cached at ~/.focal/bin/
 *   4. Download from GitHub releases
 *
 * Returns the absolute path to a verified-existing binary,
 * or undefined if resolution fails entirely.
 */
export async function resolveBinary(
  extensionPath: string
): Promise<string | undefined> {
  // 1. User-configured path
  const configured = vscode.workspace
    .getConfiguration("focal")
    .get<string>("coreBinaryPath", "");
  if (configured && fs.existsSync(configured)) {
    return configured;
  }

  // 2. Bundled binary
  const binaryName = os.platform() === "win32" ? "focal.exe" : "focal";
  const bundled = path.join(extensionPath, "bin", binaryName);
  if (fs.existsSync(bundled)) {
    return bundled;
  }

  // 3. Cached binary
  const cached = cachedBinaryPath();
  if (fs.existsSync(cached)) {
    return cached;
  }

  // 4. Download from GitHub
  return downloadBinary();
}

/**
 * Downloads the latest focal binary from GitHub releases.
 * Shows a VS Code progress notification during download.
 * Returns the path to the downloaded binary, or undefined on failure.
 */
async function downloadBinary(): Promise<string | undefined> {
  const assetName = binaryAssetName();
  if (!assetName) {
    vscode.window.showErrorMessage(
      `Focal: unsupported platform/arch: ${os.platform()}/${os.arch()}`
    );
    return undefined;
  }

  return vscode.window.withProgress(
    {
      location: vscode.ProgressLocation.Notification,
      title: "Focal: downloading binary...",
      cancellable: false,
    },
    async (progress) => {
      try {
        progress.report({ message: "Fetching release info..." });
        const release = await fetchJson<GitHubRelease>(GITHUB_API_RELEASE);

        const asset = release.assets.find((a) => a.name === assetName);
        if (!asset) {
          vscode.window.showErrorMessage(
            `Focal: release asset "${assetName}" not found. ` +
              `Download manually from ${RELEASES_URL}`
          );
          return undefined;
        }

        progress.report({ message: `Downloading ${assetName}...` });

        const destPath = cachedBinaryPath();
        const destDir = path.dirname(destPath);
        if (!fs.existsSync(destDir)) {
          fs.mkdirSync(destDir, { recursive: true });
        }

        await downloadFile(asset.browser_download_url, destPath);

        // chmod 755 on Unix
        if (os.platform() !== "win32") {
          fs.chmodSync(destPath, 0o755);
        }

        progress.report({ message: "Done" });
        vscode.window.showInformationMessage(
          `Focal: binary downloaded to ${destPath}`
        );
        return destPath;
      } catch (err) {
        const msg = err instanceof Error ? err.message : String(err);
        vscode.window.showErrorMessage(
          `Focal: failed to download binary — ${msg}`
        );
        return undefined;
      }
    }
  );
}

/**
 * Fetches JSON from a URL, following redirects.
 */
function fetchJson<T>(url: string): Promise<T> {
  return new Promise((resolve, reject) => {
    const request = https.get(
      url,
      { headers: { "User-Agent": "focal-vscode", Accept: "application/json" } },
      (res) => {
        // Follow redirects
        if (
          res.statusCode &&
          res.statusCode >= 300 &&
          res.statusCode < 400 &&
          res.headers.location
        ) {
          fetchJson<T>(res.headers.location).then(resolve, reject);
          return;
        }

        if (res.statusCode !== 200) {
          reject(new Error(`HTTP ${res.statusCode} from ${url}`));
          return;
        }

        const chunks: Buffer[] = [];
        res.on("data", (chunk: Buffer) => chunks.push(chunk));
        res.on("end", () => {
          try {
            resolve(JSON.parse(Buffer.concat(chunks).toString("utf-8")) as T);
          } catch (e) {
            reject(e);
          }
        });
        res.on("error", reject);
      }
    );
    request.on("error", reject);
  });
}

/**
 * Downloads a file from a URL to disk, following redirect chains
 * (GitHub uses 302 redirects for release asset downloads).
 */
function downloadFile(url: string, destPath: string): Promise<void> {
  return new Promise((resolve, reject) => {
    const proto = url.startsWith("https") ? https : http;
    const request = proto.get(
      url,
      { headers: { "User-Agent": "focal-vscode" } },
      (res) => {
        // Follow redirects
        if (
          res.statusCode &&
          res.statusCode >= 300 &&
          res.statusCode < 400 &&
          res.headers.location
        ) {
          downloadFile(res.headers.location, destPath).then(resolve, reject);
          return;
        }

        if (res.statusCode !== 200) {
          reject(new Error(`HTTP ${res.statusCode} downloading ${url}`));
          return;
        }

        const file = fs.createWriteStream(destPath);
        res.pipe(file);
        file.on("finish", () => {
          file.close();
          resolve();
        });
        file.on("error", (err) => {
          // Clean up partial download
          fs.unlink(destPath, () => {});
          reject(err);
        });
        res.on("error", (err) => {
          fs.unlink(destPath, () => {});
          reject(err);
        });
      }
    );
    request.on("error", reject);
  });
}
