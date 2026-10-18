const { execSync } = require('child_process')
const path = require('path')

exports.default = async function (context) {
  if (process.platform !== 'darwin') return

  const appPath = path.join(context.appOutDir, `${context.packager.appInfo.productFilename}.app`)
  console.log(`[afterPack] Post-processing: ${appPath}`)

  // 1. Strip all extended attributes (com.apple.provenance, com.apple.quarantine, etc.)
  //    This prevents Gatekeeper from blocking the app based on download quarantine flags.
  try {
    execSync(`find "${appPath}" -exec xattr -c {} \\; 2>/dev/null || true`, { stdio: 'inherit' })
    execSync(`find "${appPath}" -name "._*" -delete 2>/dev/null || true`, { stdio: 'inherit' })
    execSync(`dot_clean "${appPath}" 2>/dev/null || true`, { stdio: 'inherit' })
    console.log('[afterPack] Extended attributes stripped')
  } catch (e) {
    console.warn('[afterPack] xattr strip warning:', e.message)
  }

  // 2. Ad-hoc code sign the entire app bundle.
  //    Without a Developer ID, we use "-" (ad-hoc signature) so macOS sees a
  //    valid signature. This lets users right-click > Open without needing to
  //    go to System Preferences > Security each time.
  //    NOTE: On Apple Silicon (arm64), ad-hoc signing is REQUIRED — unsigned
  //    binaries are rejected entirely by macOS, not just quarantined.
  try {
    console.log('[afterPack] Ad-hoc signing app bundle...')
    // Sign all nested binaries/dylibs/frameworks first (deep sign order matters)
    execSync(
      `find "${appPath}/Contents" -type f \\( -name "*.dylib" -o -name "*.so" -o -name "*.node" \\) | while read f; do codesign --force --sign - "$f" 2>/dev/null || true; done`,
      { stdio: 'inherit', shell: '/bin/bash' }
    )
    // Sign frameworks
    execSync(
      `find "${appPath}/Contents/Frameworks" -name "*.framework" -prune | while read f; do codesign --force --deep --sign - "$f" 2>/dev/null || true; done`,
      { stdio: 'inherit', shell: '/bin/bash' }
    )
    // Sign the top-level app bundle
    execSync(`codesign --force --deep --sign - "${appPath}"`, { stdio: 'inherit' })
    console.log('[afterPack] Ad-hoc signing complete')
  } catch (e) {
    console.warn('[afterPack] codesign warning (non-fatal):', e.message)
  }
}
