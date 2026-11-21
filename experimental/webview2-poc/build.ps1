$ErrorActionPreference = "Stop"

$Root = Split-Path -Parent $MyInvocation.MyCommand.Path
$BuildDir = Join-Path $Root "build"
$SdkDir = Join-Path $BuildDir "sdk"
$OutExe = Join-Path $BuildDir "bg-tok-webview2-poc.exe"

New-Item -ItemType Directory -Force -Path $BuildDir | Out-Null

if (-not (Test-Path (Join-Path $SdkDir "build\native\include\WebView2.h"))) {
    New-Item -ItemType Directory -Force -Path $SdkDir | Out-Null
    $Zip = Join-Path $BuildDir "Microsoft.Web.WebView2.zip"
    Write-Host "Downloading Microsoft.Web.WebView2 NuGet package..."
    curl.exe -sSL "https://www.nuget.org/api/v2/package/Microsoft.Web.WebView2/1.0.2903.40" -o $Zip
    Expand-Archive -Path $Zip -DestinationPath $SdkDir -Force
}

$IncludeDir = Join-Path $SdkDir "build\native\include"
$LibPath = Join-Path $SdkDir "build\native\x64\WebView2LoaderStatic.lib"
$Src = Join-Path $Root "main.cpp"

cl.exe /nologo /std:c++20 /EHsc /O2 /W3 `
    /I"$IncludeDir" `
    "$Src" `
    /Fe:"$OutExe" `
    /Fo:"$BuildDir\main.obj" `
    /link /SUBSYSTEM:WINDOWS "$LibPath"
if ($LASTEXITCODE -ne 0) { throw "cl.exe failed with exit code $LASTEXITCODE" }

$TestSrc = Join-Path $Root "feed_queue_tests.cpp"
$TestExe = Join-Path $BuildDir "feed_queue_tests.exe"
cl.exe /nologo /std:c++20 /EHsc /O2 /W3 `
    "$TestSrc" `
    /Fe:"$TestExe" `
    /Fo:"$BuildDir\feed_queue_tests.obj"
if ($LASTEXITCODE -ne 0) { throw "feed_queue_tests compile failed" }
& "$TestExe"
if ($LASTEXITCODE -ne 0) { throw "feed_queue_tests failed" }

Write-Host "Built: $OutExe"
(Get-Item $OutExe) | Select-Object FullName, Length
