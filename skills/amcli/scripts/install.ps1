# Install the amcli binary on Windows.
#
# CONTRACT, matching scripts/install.sh: stdout carries the absolute path of
# the binary and nothing else, so an agent can rely on
#
#     $AMCLI = & ~\.agents\skills\amcli\scripts\install.ps1
#
# and use $AMCLI for the rest of the session. Everything a human reads goes to
# stderr, because a freshly installed binary is not on PATH until the shell is
# restarted, and "amcli" alone will still report that it is not recognised.
#
# Never elevates. Never edits the registry or a profile script.
#
#   -Version v0.1.0   install that tag instead of the newest
#   -InstallDir PATH  default %LOCALAPPDATA%\Programs\amcli
#   -DryRun           report what would happen, download nothing
#
# It trusts what install.sh trusts, and the header there says it in full: one
# hardcoded host, a tag that is checked to be a tag before it reaches a URL,
# a SHA256 check against the release's SHA256SUMS that nothing can skip, and
# only the binary taken out of the archive by name.

[CmdletBinding()]
param(
    [string]$Version,
    [string]$InstallDir = "$env:LOCALAPPDATA\Programs\amcli",
    [switch]$DryRun
)

$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'   # the progress bar corrupts a non-interactive host

# Windows PowerShell 5.1, which is what ships with Windows, still defaults to
# TLS 1.0 on many builds. github.com refuses that, so without this line the
# download fails on exactly the hosts this script exists for. PowerShell 7
# negotiates on its own and the assignment is harmless there.
try {
    [Net.ServicePointManager]::SecurityProtocol =
        [Net.ServicePointManager]::SecurityProtocol -bor [Net.SecurityProtocolType]::Tls12
} catch {}

$Repo = 'arslanr-com/amcli'
$Base = "https://github.com/$Repo"

function Say([string]$m) { [Console]::Error.WriteLine($m) }
function Die([string]$m) { [Console]::Error.WriteLine("amcli install: $m"); exit 1 }

# -Version is interpolated into a download URL, so it is checked to be a tag
# rather than passed along: a slash in it would aim the download elsewhere.
if ($Version -and $Version -notmatch '^v[0-9][A-Za-z0-9._-]*$') {
    Die "-Version must be a release tag such as v0.6.0"
}

# Windows on ARM runs x64 binaries under emulation, and there is no arm64
# build yet, so x64 is the right answer for both.
$target = 'x86_64-pc-windows-msvc'
$bin = 'amcli.exe'

# Resolve the newest tag from the redirect rather than api.github.com, whose
# unauthenticated limit of 60 requests/hour is shared across a whole NAT.
function Resolve-Tag {
    try {
        $r = Invoke-WebRequest -Uri "$Base/releases/latest" -MaximumRedirection 5 -UseBasicParsing
        # PowerShell 7 exposes the final URI here; Windows PowerShell 5.1,
        # which is what ships with Windows, exposes it there.
        $url = $r.BaseResponse.RequestMessage.RequestUri.AbsoluteUri
        if (-not $url) { $url = $r.BaseResponse.ResponseUri.AbsoluteUri }
    } catch {
        return $null
    }
    if (-not $url) { return $null }
    $tag = $url.Split('/')[-1]
    # With no releases published GitHub lands on the list page instead, so the
    # last segment is "releases". That is a fresh repository, not an error.
    if ($tag -match '^v[0-9]') { return $tag }
    return $null
}

# The version already installed at the target path, as vX.Y.Z, or $null.
function Installed-Tag {
    $exe = Join-Path $InstallDir $bin
    if (-not (Test-Path $exe)) { return $null }
    try { $out = & $exe --version 2>$null } catch { return $null }
    if ($out -match '^amcli (\S+)') { return "v$($Matches[1])" }
    return $null
}

if ($Version) { $tag = $Version } else { $tag = Resolve-Tag }

# No release, or no network: keep an installed binary rather than fail.
if (-not $tag) {
    $have = Installed-Tag
    if ($have) {
        Say "could not reach $Base; keeping installed amcli $have"
        Write-Output (Resolve-Path (Join-Path $InstallDir $bin)).Path
        exit 0
    }
}

# Already current: nothing to download.
if ($tag -and (Installed-Tag) -eq $tag) {
    Say "amcli $tag is already installed"
    Write-Output (Resolve-Path (Join-Path $InstallDir $bin)).Path
    exit 0
}

if (-not $tag) {
    if ($DryRun) {
        Say "target:  $target"
        Say "release: none published yet, would build with cargo"
        Write-Output (Join-Path $InstallDir $bin)
        exit 0
    }
    if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) {
        Die @"
no published release yet and cargo is not installed. Install Rust from
https://rustup.rs and re-run this script, or run:
  cargo install --git $Base --locked amcli-cli
"@
    }
    Say 'no published release found; building from source with cargo (a few minutes)...'
    $stage = Join-Path $InstallDir ".amcli-install-$([System.Guid]::NewGuid().ToString('N'))"
    # Everything cargo prints has to reach stderr, or the build log would land
    # on stdout and break the one-line contract. PowerShell has no `1>&2` —
    # that operator is reserved — so route both streams by hand.
    cargo install --git $Base --locked --root $stage amcli-cli 2>&1 |
        ForEach-Object { [Console]::Error.WriteLine($_) }
    if ($LASTEXITCODE -ne 0) {
        Die 'cargo could not build amcli. It needs Rust 1.90 or newer (edition 2024); run `rustup update` if yours is older.'
    }
    New-Item -ItemType Directory -Force -Path $InstallDir | Out-Null
    Move-Item -Force (Join-Path $stage "bin\$bin") (Join-Path $InstallDir $bin)
    Remove-Item -Recurse -Force $stage
    $resolved = (Resolve-Path (Join-Path $InstallDir $bin)).Path
    Say "installed $(& $resolved --version)"
    Write-Output $resolved
    exit 0
}

$asset = "amcli-$tag-$target.tar.gz"
$url = "$Base/releases/download/$tag/$asset"

if ($DryRun) {
    Say "target:  $target"
    Say "release: $tag"
    Say "url:     $url"
    Say "install: $(Join-Path $InstallDir $bin)"
    Write-Output (Join-Path $InstallDir $bin)
    exit 0
}

New-Item -ItemType Directory -Force -Path $InstallDir | Out-Null
# Staged inside the install directory so the final move is a rename on the
# same volume and two installs cannot interleave.
$tmp = Join-Path $InstallDir ".amcli-install-$([System.Guid]::NewGuid().ToString('N'))"
New-Item -ItemType Directory -Force -Path $tmp | Out-Null

try {
    Say "downloading amcli $tag for $target..."
    try {
        Invoke-WebRequest -Uri $url -OutFile (Join-Path $tmp $asset) -UseBasicParsing
        Invoke-WebRequest -Uri "$Base/releases/download/$tag/SHA256SUMS" -OutFile (Join-Path $tmp 'SHA256SUMS') -UseBasicParsing
    } catch {
        Die "could not download $asset from release $tag ($($_.Exception.Message))"
    }

    $want = (Get-Content (Join-Path $tmp 'SHA256SUMS') |
        Where-Object { ($_ -split '\s+')[1] -eq $asset } |
        ForEach-Object { ($_ -split '\s+')[0] } |
        Select-Object -First 1)
    if (-not $want) { Die "$asset is not listed in SHA256SUMS" }

    $got = (Get-FileHash -Algorithm SHA256 -Path (Join-Path $tmp $asset)).Hash
    if ($got -ne $want.ToUpperInvariant()) {
        Die "checksum mismatch for $asset`n  expected $want`n  got      $got"
    }

    # tar.exe has shipped in Windows since 10 1803, so the release uses one
    # archive format for every platform.
    #
    # Verified before it is opened, and only the member named here comes out
    # of it, so nothing else the archive carries is ever written to disk.
    tar -xzf (Join-Path $tmp $asset) -C $tmp $bin
    if ($LASTEXITCODE -ne 0) { Die "could not unpack $bin from $asset" }
    if (-not (Test-Path (Join-Path $tmp $bin))) { Die "$asset does not contain $bin" }

    Move-Item -Force (Join-Path $tmp $bin) (Join-Path $InstallDir $bin)
} finally {
    if (Test-Path $tmp) { Remove-Item -Recurse -Force $tmp }
}

$resolved = (Resolve-Path (Join-Path $InstallDir $bin)).Path

$onPath = (Get-Command amcli -ErrorAction SilentlyContinue).Source
if ($onPath -and $onPath -ne $resolved) {
    Say "warning: $onPath comes earlier in PATH and will shadow this install."
    Say '         Use the absolute path below, or remove the other copy.'
} elseif (-not $onPath) {
    Say "note: $InstallDir is not on PATH. Use the absolute path below, or add it:"
    Say "         setx PATH `"$InstallDir;`$env:PATH`""
}

Say "installed $(& $resolved --version)"
Write-Output $resolved
