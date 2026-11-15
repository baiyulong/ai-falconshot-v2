$ErrorActionPreference = 'Stop'

# P213: 把用户的 logo（assets/Square150x150Logo.png，2026-11-13 放进来 ✓）打成一枚多尺寸
# 的 .ico，交给 qt_bridge 的 build.rs 经 rc.exe 嵌成 exe 的图标资源 #1 —— 托盘那一枚脸
# （platform-windows/src/hotkey.rs 的 pump_thread 从本进程模块取）、资源管理器里的文件
# 图标、任务栏上窗口没自设图标时的默认，取的都是它。
#
# 32bpp BGRA + AND 掩码（全 0，透明交给 alpha 通道），纯手工打包：ICO 目录 + BITMAPINFOHEADER
# + 自下而上的像素平面。不用 PNG 压缩条目（rc.exe 收的就是最朴素的那一种 BMP 条目 ✓）。

Add-Type -AssemblyName System.Drawing

$srcPath = Join-Path $PSScriptRoot 'assets\Square150x150Logo.png'
$outPath = Join-Path (Split-Path $PSScriptRoot) 'crates\qt_bridge\assets\app.ico'
$sizes = @(16, 24, 32, 48, 64, 128, 256)

$src = [System.Drawing.Bitmap]::new($srcPath)
$parts = New-Object System.Collections.Generic.List[object]
foreach ($s in $sizes) {
    $bmp = [System.Drawing.Bitmap]::new($s, $s, [System.Drawing.Imaging.PixelFormat]::Format32bppArgb)
    $g = [System.Drawing.Graphics]::FromImage($bmp)
    try {
        $g.InterpolationMode = [System.Drawing.Drawing2D.InterpolationMode]::HighQualityBicubic
        $g.SmoothingMode = [System.Drawing.Drawing2D.SmoothingMode]::HighQuality
        $g.PixelOffsetMode = [System.Drawing.Drawing2D.PixelOffsetMode]::HighQuality
        $g.CompositingQuality = [System.Drawing.Drawing2D.CompositingQuality]::HighQuality
        $g.Clear([System.Drawing.Color]::Transparent)
        $g.DrawImage($src,
            [System.Drawing.RectangleF]::new(0, 0, $s, $s),
            [System.Drawing.RectangleF]::new(0, 0, $src.Width, $src.Height),
            [System.Drawing.GraphicsUnit]::Pixel)
    }
    finally {
        $g.Dispose()
    }

    $rect = [System.Drawing.Rectangle]::new(0, 0, $s, $s)
    $data = $bmp.LockBits($rect, [System.Drawing.Imaging.ImageLockMode]::ReadOnly, [System.Drawing.Imaging.PixelFormat]::Format32bppArgb)
    $stride = $data.Stride
    $raw = [byte[]]::new($stride * $s)
    [System.Runtime.InteropServices.Marshal]::Copy($data.Scan0, $raw, 0, $raw.Length)
    $bmp.UnlockBits($data)
    $bmp.Dispose()

    # ICO 的 XOR 平面要求自下而上；32bppArgb 锁出来的内存布局本来就是 BGRA ✓
    $xor = [byte[]]::new($stride * $s)
    for ($y = 0; $y -lt $s; $y++) {
        [Array]::Copy($raw, ($s - 1 - $y) * $stride, $xor, $y * $stride, $stride)
    }
    # AND 掩码：每行 ceil(s/8) 字节再补到 4 的倍数；全 0（透明由 alpha 通道说了算）
    $andRow = [int][Math]::Floor(($s + 31) / 32) * 4
    $and = [byte[]]::new($andRow * $s)
    $parts.Add([pscustomobject]@{ Size = $s; Xor = $xor; And = $and })
}
$src.Dispose()

$ms = [System.IO.MemoryStream]::new()
$w = [System.IO.BinaryWriter]::new($ms)
try {
    # ---- 目录：6 字节头 + 每枚 16 字节 ----
    $w.Write([uint16]0)                 # reserved
    $w.Write([uint16]1)                 # type: icon
    $w.Write([uint16]$parts.Count)
    $offset = 6 + 16 * $parts.Count
    foreach ($p in $parts) {
        $s = $p.Size
        $b = if ($s -ge 256) { [byte]0 } else { [byte]$s }   # 256 在目录里写作 0
        $w.Write($b); $w.Write($b)
        $w.Write([byte]0)               # 调色板色数
        $w.Write([byte]0)               # reserved
        $w.Write([uint16]1)             # planes
        $w.Write([uint16]32)            # bpp
        $w.Write([uint32](40 + $p.Xor.Length + $p.And.Length))
        $w.Write([uint32]$offset)
        $offset += 40 + $p.Xor.Length + $p.And.Length
    }
    # ---- 条目：BITMAPINFOHEADER + XOR + AND ----
    foreach ($p in $parts) {
        $s = $p.Size
        $w.Write([uint32]40)            # biSize
        $w.Write([int32]$s)             # biWidth
        $w.Write([int32]($s * 2))       # biHeight：XOR + AND 两个平面叠起来
        $w.Write([uint16]1)             # biPlanes
        $w.Write([uint16]32)            # biBitCount
        $w.Write([uint32]0)             # biCompression = BI_RGB
        $w.Write([uint32]($p.Xor.Length + $p.And.Length))
        $w.Write([int32]0); $w.Write([int32]0)   # ppm
        $w.Write([uint32]0); $w.Write([uint32]0) # 颜色表
        $w.Write([byte[]]$p.Xor)
        $w.Write([byte[]]$p.And)
    }
    $w.Flush()
    New-Item -ItemType Directory -Path (Split-Path $outPath) -Force | Out-Null
    [System.IO.File]::WriteAllBytes($outPath, $ms.ToArray())
}
finally {
    $w.Dispose()
}
Write-Output ("app.ico 写出：{0}  {1:N0} 字节  尺寸 {2}  共 {3} 枚" -f $outPath, $ms.Length, ($sizes -join '/'), $parts.Count)
