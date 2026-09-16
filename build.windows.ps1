# ============================================================
# PIC - Portable Windows Runtime Builder
# ============================================================
#
# Put this script in the SAME folder as the PIC EXE.
#
# EXE name does not matter:
#   pic-rs.exe
#   picasa-rs.exe
#   PIC.exe
#   etc.
#
# Run:
#   powershell.exe -ExecutionPolicy Bypass -File .\build.windows.ps1
#
# Requires:
#   C:\msys64\ucrt64
#
# Creates:
#   build.windows.log
#
# ============================================================

$ErrorActionPreference = "Stop"

# ------------------------------------------------------------
# Basic paths
# ------------------------------------------------------------

$AppDir = (Get-Location).Path
$Ucrt   = "C:\msys64\ucrt64"
$Bin    = Join-Path $Ucrt "bin"
$Share  = Join-Path $Ucrt "share"
$Lib    = Join-Path $Ucrt "lib"

# ------------------------------------------------------------
# Start fresh log
# ------------------------------------------------------------

$LogFile = Join-Path $AppDir "build.windows.log"

if (Test-Path $LogFile) {
    Remove-Item $LogFile -Force
}

Start-Transcript -Path $LogFile -Force

Write-Host ""
Write-Host "============================================================"
Write-Host " PIC PORTABLE WINDOWS RUNTIME BUILDER"
Write-Host "============================================================"
Write-Host ""
Write-Host "Folder : $AppDir"
Write-Host "MSYS2  : $Ucrt"
Write-Host "Log    : $LogFile"
Write-Host ""

try {

    # --------------------------------------------------------
    # Check MSYS2
    # --------------------------------------------------------

    if (!(Test-Path $Ucrt)) {
        throw "MSYS2 UCRT64 not found: $Ucrt"
    }

    if (!(Test-Path $Bin)) {
        throw "MSYS2 UCRT64 bin directory not found: $Bin"
    }

    # --------------------------------------------------------
    # Find application EXE
    # --------------------------------------------------------

    $ExeFiles = @(
        Get-ChildItem -Path $AppDir -Filter "*.exe" -File |
        Where-Object {
            $_.Name -notmatch "^(gdb|gspawn|gtk|glib|gdk|gst|fc-|gio-|gsettings)"
        }
    )

    if ($ExeFiles.Count -eq 0) {
        throw "No application EXE found in current folder."
    }

    if ($ExeFiles.Count -gt 1) {

        Write-Host "More than one application EXE found:"
        Write-Host ""

        foreach ($item in $ExeFiles) {
            Write-Host "  $($item.Name)"
        }

        throw "Keep only the PIC application EXE in this folder and run again."
    }

    $Exe = $ExeFiles[0]

    Write-Host "Application EXE:"
    Write-Host "  $($Exe.Name)"
    Write-Host ""

    # --------------------------------------------------------
    # Find ldd
    # --------------------------------------------------------

    $LddCandidates = @(
        "C:\msys64\usr\bin\ldd.exe",
        "C:\msys64\ucrt64\bin\ldd.exe"
    )

    $Ldd = $null

    foreach ($candidate in $LddCandidates) {

        if (Test-Path $candidate) {
            $Ldd = $candidate
            break
        }
    }

    if (!$Ldd) {
        throw "Could not find ldd.exe in MSYS2."
    }

    Write-Host "Dependency scanner:"
    Write-Host "  $Ldd"
    Write-Host ""

    # --------------------------------------------------------
    # Save original PATH
    # --------------------------------------------------------

    $OldPath = $env:PATH

    # Needed while collecting dependencies
    $env:PATH = "$Bin;$OldPath"

    # --------------------------------------------------------
    # Scan dependency tree
    # --------------------------------------------------------

    Write-Host "============================================================"
    Write-Host " SCANNING DLL DEPENDENCIES"
    Write-Host "============================================================"
    Write-Host ""

    $LddOutput = & $Ldd $Exe.FullName 2>&1

    $DllNames = New-Object System.Collections.Generic.HashSet[string]

    foreach ($line in $LddOutput) {

        $matchesFound = [regex]::Matches(
            $line,
            '(?i)([A-Za-z0-9_.+\-]+\.dll)'
        )

        foreach ($m in $matchesFound) {
            [void]$DllNames.Add($m.Groups[1].Value)
        }
    }

    Write-Host "Dependencies detected: $($DllNames.Count)"
    Write-Host ""

    # --------------------------------------------------------
    # Windows DLLs
    #
    # These belong to Windows.
    # DO NOT copy them from MSYS2.
    # --------------------------------------------------------

    $WindowsSystemDlls = @(
        "advapi32.dll",
        "bcrypt.dll",
        "bcryptprimitives.dll",
        "cfgmgr32.dll",
        "combase.dll",
        "comctl32.dll",
        "comdlg32.dll",
        "crypt32.dll",
        "d3d11.dll",
        "d3d12.dll",
        "dcomp.dll",
        "dnsapi.dll",
        "dpapi.dll",
        "dsparse.dll",
        "dwmapi.dll",
        "dwrite.dll",
        "dxcore.dll",
        "dxgi.dll",
        "gdi32.dll",
        "gdi32full.dll",
        "gdiplus.dll",
        "glu32.dll",
        "hid.dll",
        "imm32.dll",
        "iphlpapi.dll",
        "kernel.appcore.dll",
        "kernel32.dll",
        "kernelbase.dll",
        "microsoft.internal.warppal.dll",
        "msimg32.dll",
        "msvcp_win.dll",
        "msvcrt.dll",
        "ntdll.dll",
        "ole32.dll",
        "oleaut32.dll",
        "opengl32.dll",
        "powrprof.dll",
        "rpcrt4.dll",
        "sechost.dll",
        "secur32.dll",
        "setupapi.dll",
        "shell32.dll",
        "shcore.dll",
        "shlwapi.dll",
        "sspicli.dll",
        "ucrtbase.dll",
        "user32.dll",
        "usp10.dll",
        "uxtheme.dll",
        "version.dll",
        "win32u.dll",
        "winmm.dll",
        "wldap32.dll",
        "ws2_32.dll"
    )

    $WindowsLookup = @{}

    foreach ($dll in $WindowsSystemDlls) {
        $WindowsLookup[$dll.ToLower()] = $true
    }

    # --------------------------------------------------------
    # Copy DLL dependencies
    # --------------------------------------------------------

    Write-Host "============================================================"
    Write-Host " COPYING DLLS"
    Write-Host "============================================================"
    Write-Host ""

    $CopiedDLL = 0
    $WindowsDLLCount = 0
    $MissingDLLs = New-Object System.Collections.Generic.HashSet[string]

    foreach ($DllName in ($DllNames | Sort-Object)) {

        $Lower = $DllName.ToLower()

        # Windows provides this DLL
        if ($WindowsLookup.ContainsKey($Lower)) {

            Write-Host "WINDOWS $DllName"
            $WindowsDLLCount++
            continue
        }

        $Source = Join-Path $Bin $DllName
        $Dest   = Join-Path $AppDir $DllName

        if (Test-Path $Source) {

            Copy-Item $Source $Dest -Force

            Write-Host "DLL     $DllName"
            $CopiedDLL++
        }
        elseif (Test-Path $Dest) {

            Write-Host "EXISTS  $DllName"
        }
        else {

            Write-Host "MISSING $DllName"
            [void]$MissingDLLs.Add($DllName)
        }
    }

    # --------------------------------------------------------
    # Directory copy helper
    # --------------------------------------------------------

    function Copy-PicRuntimeDirectory {

        param(
            [string]$Source,
            [string]$Destination
        )

        if (Test-Path $Source) {

            Write-Host ""
            Write-Host "COPY:"
            Write-Host "  $Source"
            Write-Host "  -> $Destination"

            New-Item `
                -ItemType Directory `
                -Force `
                -Path $Destination |
                Out-Null

            Copy-Item `
                "$Source\*" `
                $Destination `
                -Recurse `
                -Force
        }
        else {

            Write-Host ""
            Write-Host "NOT PRESENT IN MSYS2:"
            Write-Host "  $Source"
        }
    }

    # --------------------------------------------------------
    # GTK / GLib / libadwaita
    # --------------------------------------------------------

    Write-Host ""
    Write-Host "============================================================"
    Write-Host " GTK / LIBADWAITA RUNTIME"
    Write-Host "============================================================"

    # GSettings
    Copy-PicRuntimeDirectory `
        "$Share\glib-2.0\schemas" `
        "$AppDir\share\glib-2.0\schemas"

    # Adwaita icons
    Copy-PicRuntimeDirectory `
        "$Share\icons\Adwaita" `
        "$AppDir\share\icons\Adwaita"

    # hicolor fallback icons
    Copy-PicRuntimeDirectory `
        "$Share\icons\hicolor" `
        "$AppDir\share\icons\hicolor"

    # GTK runtime data
    Copy-PicRuntimeDirectory `
        "$Share\gtk-4.0" `
        "$AppDir\share\gtk-4.0"

    # libadwaita data - may not exist as a separate directory
    Copy-PicRuntimeDirectory `
        "$Share\libadwaita-1" `
        "$AppDir\share\libadwaita-1"

    # Themes - may not exist on current MSYS2 GTK4 install
    Copy-PicRuntimeDirectory `
        "$Share\themes" `
        "$AppDir\share\themes"

    # --------------------------------------------------------
    # GDK Pixbuf
    # --------------------------------------------------------

    Write-Host ""
    Write-Host "============================================================"
    Write-Host " GDK PIXBUF"
    Write-Host "============================================================"

    Copy-PicRuntimeDirectory `
        "$Lib\gdk-pixbuf-2.0" `
        "$AppDir\lib\gdk-pixbuf-2.0"

    # --------------------------------------------------------
    # GStreamer plugins
    # --------------------------------------------------------

    Write-Host ""
    Write-Host "============================================================"
    Write-Host " GSTREAMER"
    Write-Host "============================================================"

    Copy-PicRuntimeDirectory `
        "$Lib\gstreamer-1.0" `
        "$AppDir\lib\gstreamer-1.0"

    # --------------------------------------------------------
    # GStreamer helpers
    # --------------------------------------------------------

    Write-Host ""
    Write-Host "GStreamer helper executables:"
    Write-Host ""

    $GstHelpers = @(
        "gst-plugin-scanner.exe",
        "gst-ptp-helper.exe"
    )

    foreach ($Helper in $GstHelpers) {

        $Source = Join-Path $Bin $Helper

        if (Test-Path $Source) {

            Copy-Item `
                $Source `
                (Join-Path $AppDir $Helper) `
                -Force

            Write-Host "HELPER  $Helper"
        }
        else {
            Write-Host "SKIP    $Helper"
        }
    }

    # --------------------------------------------------------
    # MIME database
    # --------------------------------------------------------

    Write-Host ""
    Write-Host "============================================================"
    Write-Host " MIME DATABASE"
    Write-Host "============================================================"

    Copy-PicRuntimeDirectory `
        "$Share\mime" `
        "$AppDir\share\mime"

    # --------------------------------------------------------
    # FontConfig
    # --------------------------------------------------------

    Write-Host ""
    Write-Host "============================================================"
    Write-Host " FONTCONFIG"
    Write-Host "============================================================"

    Copy-PicRuntimeDirectory `
        "$Ucrt\etc\fonts" `
        "$AppDir\etc\fonts"

    Copy-PicRuntimeDirectory `
        "$Share\fontconfig" `
        "$AppDir\share\fontconfig"

    # --------------------------------------------------------
    # Locales
    # --------------------------------------------------------

    Write-Host ""
    Write-Host "============================================================"
    Write-Host " LOCALES"
    Write-Host "============================================================"

    Copy-PicRuntimeDirectory `
        "$Share\locale" `
        "$AppDir\share\locale"

    # --------------------------------------------------------
    # Compile GSettings schemas
    # --------------------------------------------------------

    Write-Host ""
    Write-Host "============================================================"
    Write-Host " GSTTINGS SCHEMAS"
    Write-Host "============================================================"
    Write-Host ""

    $SchemaDir      = "$AppDir\share\glib-2.0\schemas"
    $SchemaCompiler = "$Bin\glib-compile-schemas.exe"

    if (
        (Test-Path $SchemaCompiler) -and
        (Test-Path $SchemaDir)
    ) {

        & $SchemaCompiler $SchemaDir

        if ($LASTEXITCODE -eq 0) {
            Write-Host "GSettings schemas compiled successfully."
        }
        else {
            Write-Host "WARNING: GSettings schema compiler returned an error."
        }
    }
    else {

        Write-Host "WARNING: Cannot compile GSettings schemas."
    }

    # --------------------------------------------------------
    # Restore normal PATH before validation
    # --------------------------------------------------------

    $env:PATH = $OldPath

    # --------------------------------------------------------
    # PORTABLE RUNTIME VALIDATION
    # --------------------------------------------------------

    Write-Host ""
    Write-Host "============================================================"
    Write-Host " PIC PORTABLE RUNTIME VALIDATION"
    Write-Host "============================================================"
    Write-Host ""

    $ValidationFailed = $false

    function Test-PicRuntime {

        param(
            [string]$Name,
            [string]$RuntimePath,
            [bool]$Required = $true
        )

        if (
            $RuntimePath -and
            (Test-Path $RuntimePath)
        ) {

            Write-Host ("{0,-30} OK" -f $Name)
            return $true
        }

        if ($Required) {

            Write-Host ("{0,-30} MISSING" -f $Name)
            $script:ValidationFailed = $true
        }
        else {

            Write-Host ("{0,-30} NOT PRESENT / OPTIONAL" -f $Name)
        }

        return $false
    }

    Test-PicRuntime `
        "Application EXE" `
        $Exe.FullName | Out-Null

    Test-PicRuntime `
        "GTK4" `
        "$AppDir\libgtk-4-1.dll" | Out-Null

    Test-PicRuntime `
        "libadwaita" `
        "$AppDir\libadwaita-1-0.dll" | Out-Null

    Test-PicRuntime `
        "GLib" `
        "$AppDir\libglib-2.0-0.dll" | Out-Null

    Test-PicRuntime `
        "GObject" `
        "$AppDir\libgobject-2.0-0.dll" | Out-Null

    Test-PicRuntime `
        "GIO" `
        "$AppDir\libgio-2.0-0.dll" | Out-Null

    Test-PicRuntime `
        "Adwaita icons" `
        "$AppDir\share\icons\Adwaita" | Out-Null

    Test-PicRuntime `
        "hicolor icons" `
        "$AppDir\share\icons\hicolor" | Out-Null

    Test-PicRuntime `
        "GSettings schemas" `
        "$AppDir\share\glib-2.0\schemas\gschemas.compiled" | Out-Null

    Test-PicRuntime `
        "GDK Pixbuf" `
        "$AppDir\lib\gdk-pixbuf-2.0" | Out-Null

    Test-PicRuntime `
        "GStreamer plugins" `
        "$AppDir\lib\gstreamer-1.0" | Out-Null

    Test-PicRuntime `
        "FontConfig" `
        "$AppDir\etc\fonts" | Out-Null

    Test-PicRuntime `
        "MIME database" `
        "$AppDir\share\mime" | Out-Null

    Test-PicRuntime `
        "Locales" `
        "$AppDir\share\locale" | Out-Null

    # These are optional because your MSYS2 installation already
    # showed that they do not exist separately.

    Test-PicRuntime `
        "Separate libadwaita data" `
        "$AppDir\share\libadwaita-1" `
        $false | Out-Null

    Test-PicRuntime `
        "Separate themes folder" `
        "$AppDir\share\themes" `
        $false | Out-Null

    # --------------------------------------------------------
    # Check copied DLLs
    # --------------------------------------------------------

    Write-Host ""
    Write-Host "------------------------------------------------------------"
    Write-Host " DLL RESULTS"
    Write-Host "------------------------------------------------------------"
    Write-Host ""

    if ($MissingDLLs.Count -eq 0) {

        Write-Host "Missing packaged DLLs        0"
    }
    else {

        Write-Host "Missing packaged DLLs        $($MissingDLLs.Count)"

        foreach ($dll in ($MissingDLLs | Sort-Object)) {
            Write-Host "  MISSING: $dll"
        }

        $ValidationFailed = $true
    }

    Write-Host "Windows system DLLs          $WindowsDLLCount"

    # --------------------------------------------------------
    # Check for critical DLLs explicitly
    # --------------------------------------------------------

    $CriticalDlls = @(
        "libgtk-4-1.dll",
        "libadwaita-1-0.dll",
        "libglib-2.0-0.dll",
        "libgobject-2.0-0.dll",
        "libgio-2.0-0.dll",
        "libgdk_pixbuf-2.0-0.dll",
        "libcairo-2.dll",
        "libpango-1.0-0.dll",
        "libpangocairo-1.0-0.dll",
        "libgraphene-1.0-0.dll"
    )

    Write-Host ""
    Write-Host "------------------------------------------------------------"
    Write-Host " CRITICAL GTK DLL CHECK"
    Write-Host "------------------------------------------------------------"
    Write-Host ""

    foreach ($CriticalDll in $CriticalDlls) {

        $CriticalPath = Join-Path $AppDir $CriticalDll

        if (Test-Path $CriticalPath) {

            Write-Host ("{0,-30} OK" -f $CriticalDll)
        }
        else {

            Write-Host ("{0,-30} MISSING" -f $CriticalDll)
            $ValidationFailed = $true
        }
    }

    # --------------------------------------------------------
    # Package statistics
    # --------------------------------------------------------

    $DLLCount = @(
        Get-ChildItem `
            -Path $AppDir `
            -Filter "*.dll" `
            -File
    ).Count

    $AllFiles = @(
        Get-ChildItem `
            -Path $AppDir `
            -Recurse `
            -File
    )

    $FileCount = $AllFiles.Count

    $TotalSize = (
        $AllFiles |
        Measure-Object Length -Sum
    ).Sum

    $TotalSizeMB = [math]::Round(
        $TotalSize / 1MB,
        1
    )

    # --------------------------------------------------------
    # Final report
    # --------------------------------------------------------

    Write-Host ""
    Write-Host "============================================================"
    Write-Host " PIC WINDOWS PACKAGE REPORT"
    Write-Host "============================================================"
    Write-Host ""

    Write-Host ("{0,-30} {1}" -f "EXE", $Exe.Name)
    Write-Host ("{0,-30} {1}" -f "DLLs", $DLLCount)
    Write-Host ("{0,-30} {1}" -f "Total files", $FileCount)
    Write-Host ("{0,-30} {1} MB" -f "Distribution size", $TotalSizeMB)
    Write-Host ("{0,-30} {1}" -f "DLL dependencies copied", $CopiedDLL)
    Write-Host ("{0,-30} {1}" -f "Windows DLLs ignored", $WindowsDLLCount)
    Write-Host ("{0,-30} {1}" -f "Missing packaged DLLs", $MissingDLLs.Count)

    Write-Host ""
    Write-Host "============================================================"

    if ($ValidationFailed) {

        Write-Host " PORTABLE BUILD: FAILED"
        Write-Host "============================================================"
        Write-Host ""
        Write-Host "One or more required runtime components are missing."
        Write-Host ""
        Write-Host "Read:"
        Write-Host "  $LogFile"
    }
    else {

        Write-Host " PORTABLE BUILD: PASS"
        Write-Host "============================================================"
        Write-Host ""
        Write-Host "GTK/libadwaita portable runtime files are present."
        Write-Host ""
        Write-Host "IMPORTANT:"
        Write-Host "PASS means the package structure looks complete."
        Write-Host "The definitive test is still to run the Distr folder"
        Write-Host "on Windows without MSYS2 installed."
    }

    Write-Host ""
    Write-Host "Log saved to:"
    Write-Host "  $LogFile"
    Write-Host ""

}
catch {

    Write-Host ""
    Write-Host "============================================================"
    Write-Host " BUILD ERROR"
    Write-Host "============================================================"
    Write-Host ""
    Write-Host $_.Exception.Message
    Write-Host ""
    Write-Host "Full error:"
    Write-Host $_
    Write-Host ""

    if ($OldPath) {
        $env:PATH = $OldPath
    }
}
finally {

    # Always restore PATH
    if ($OldPath) {
        $env:PATH = $OldPath
    }

    Write-Host ""
    Write-Host "============================================================"
    Write-Host " Finished"
    Write-Host "============================================================"
    Write-Host ""

    Write-Host "Log:"
    Write-Host "  $LogFile"
    Write-Host ""

    try {
        Stop-Transcript
    }
    catch {
        # Do not turn a successful build into an error merely because
        # PowerShell could not stop the transcript.
    }
}