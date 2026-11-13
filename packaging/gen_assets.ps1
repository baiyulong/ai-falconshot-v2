$ErrorActionPreference = 'Stop'

# Build-time asset generator for the MSIX tile logos.
#
# makeappx refuses a package whose manifest points at logo files that are not in
# the payload, so the four sizes have to exist before the first pack.
#
# **它只补缺的，不覆盖已经有的**（2026-11-13）：仓库里那几枚是真图（`assets/` 下那四枚 PNG ✓），
# 跑这个脚本**不能**把它们换成占位小图 —— 那才是「打包出来还是占位标」的最短路径。
#
# 这不是产品自己的位图通路。计划 3.3 把每一处产品内的编码（截图、剪贴板、导出）都留在 Rust 的
# `image` crate 里；住在这里的是一个**装机器上的**资源，它从不碰任何一张被截下来的像素。
#
# 补出来的那一枚仍是**占位标**（一块平板 + 头一个字母，画在 4 倍再缩下来：44 像素的瓷砖若从 44
# 像素的源缩下来，字会被抗锯齿吃掉），不是成品商标。

Add-Type -AssemblyName System.Drawing

$out = Join-Path $PSScriptRoot 'assets'
if (-not (Test-Path $out)) { New-Item -ItemType Directory -Path $out | Out-Null }

$targets = @(
    @{ Name = 'StoreLogo.png';         Width = 50;  Height = 50 },
    @{ Name = 'Square44x44Logo.png';   Width = 44;  Height = 44 },
    @{ Name = 'Square150x150Logo.png'; Width = 150; Height = 150 },
    @{ Name = 'Wide310x150Logo.png';  Width = 310; Height = 150 }
)

foreach ($t in $targets) {
    $path = Join-Path $out $t.Name
    if (Test-Path $path) {
        Write-Output ("asset " + $t.Name + " 已在，不覆盖")
        continue
    }
    $w = $t.Width
    $h = $t.Height
    $scale = 4
    $bmp = New-Object System.Drawing.Bitmap ($w * $scale), ($h * $scale)
    $g = [System.Drawing.Graphics]::FromImage($bmp)
    try {
        $g.InterpolationMode = [System.Drawing.Drawing2D.InterpolationMode]::HighQualityBicubic
        $g.SmoothingMode = [System.Drawing.Drawing2D.SmoothingMode]::AntiAlias
        $g.TextRenderingHint = [System.Drawing.Text.TextRenderingHint]::AntiAlias

        $bg = [System.Drawing.SolidBrush]::new([System.Drawing.Color]::FromArgb(255, 24, 26, 32))
        $g.FillRectangle($bg, 0, 0, $w * $scale, $h * $scale)
        $bg.Dispose()

        # The selection rectangle: the one shape that says "screenshot" at 44 px.
        # 边长取**短边**（宽的那一枚是横着的砖，标记居中摆着 ✓）。
        $side = [math]::Min($w, $h)
        $pen = [System.Drawing.Pen]::new([System.Drawing.Color]::FromArgb(255, 0, 199, 0), [single]($side * $scale / 22))
        $m = $side * $scale * 0.22
        $ox = ($w - $side) * $scale / 2
        $oy = ($h - $side) * $scale / 2
        $g.DrawRectangle($pen, $ox + $m, $oy + $m, ($side * $scale - 2 * $m), ($side * $scale - 2 * $m))
        $pen.Dispose()

        $font = [System.Drawing.Font]::new('Segoe UI', [single]($side * $scale * 0.34), [System.Drawing.FontStyle]::Bold)
        $fg = [System.Drawing.SolidBrush]::new([System.Drawing.Color]::FromArgb(255, 235, 238, 245))
        $fmt = New-Object System.Drawing.StringFormat
        $fmt.Alignment = [System.Drawing.StringAlignment]::Center
        $fmt.LineAlignment = [System.Drawing.StringAlignment]::Center
        $rect = New-Object System.Drawing.RectangleF 0, 0, ($w * $scale), ($h * $scale)
        $g.DrawString('F', $font, $fg, $rect, $fmt)
        $font.Dispose()
        $fg.Dispose()

        $small = $bmp.GetThumbnailImage($w, $h, { $false }, [IntPtr]::Zero)
        $small.Save($path, [System.Drawing.Imaging.ImageFormat]::Png)
        $small.Dispose()
        Write-Output ("asset " + $t.Name + " " + $w + "x" + $h + " " + (Get-Item $path).Length + " B")
    }
    finally {
        $g.Dispose()
        $bmp.Dispose()
    }
}
