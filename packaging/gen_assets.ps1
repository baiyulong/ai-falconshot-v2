$ErrorActionPreference = 'Stop'

# Build-time asset generator for the MSIX tile logos.
#
# makeappx refuses a package whose manifest points at logo files that are not in
# the payload, so the three sizes have to exist before the first pack. They are a
# placeholder mark, not a finished brand: a flat panel with the initial, drawn at
# 4x and scaled down, because a 44-pixel tile made from a 44-pixel source loses
# the counter to antialiasing.
#
# This is not the product's bitmap path. Plan 3.3 keeps every product encode
# (screenshot, clipboard, export) in the Rust `image` crate; what lives here is an
# install-time asset that never touches a captured pixel.

Add-Type -AssemblyName System.Drawing

$out = Join-Path $PSScriptRoot 'assets'
if (-not (Test-Path $out)) { New-Item -ItemType Directory -Path $out | Out-Null }

$targets = @(
    @{ Name = 'StoreLogo.png';        Size = 50 },
    @{ Name = 'Square44x44Logo.png';  Size = 44 },
    @{ Name = 'Square150x150Logo.png'; Size = 150 }
)

foreach ($t in $targets) {
    $s = $t.Size
    $scale = 4
    $bmp = New-Object System.Drawing.Bitmap ($s * $scale), ($s * $scale)
    $g = [System.Drawing.Graphics]::FromImage($bmp)
    try {
        $g.InterpolationMode = [System.Drawing.Drawing2D.InterpolationMode]::HighQualityBicubic
        $g.SmoothingMode = [System.Drawing.Drawing2D.SmoothingMode]::AntiAlias
        $g.TextRenderingHint = [System.Drawing.Text.TextRenderingHint]::AntiAlias

        $bg = [System.Drawing.SolidBrush]::new([System.Drawing.Color]::FromArgb(255, 24, 26, 32))
        $g.FillRectangle($bg, 0, 0, $s * $scale, $s * $scale)
        $bg.Dispose()

        # The selection rectangle: the one shape that says "screenshot" at 44 px.
        $pen = [System.Drawing.Pen]::new([System.Drawing.Color]::FromArgb(255, 0, 199, 0), [single]($s * $scale / 22))
        $m = $s * $scale * 0.22
        $g.DrawRectangle($pen, $m, $m, ($s * $scale - 2 * $m), ($s * $scale - 2 * $m))
        $pen.Dispose()

        $font = [System.Drawing.Font]::new('Segoe UI', [single]($s * $scale * 0.34), [System.Drawing.FontStyle]::Bold)
        $fg = [System.Drawing.SolidBrush]::new([System.Drawing.Color]::FromArgb(255, 235, 238, 245))
        $fmt = New-Object System.Drawing.StringFormat
        $fmt.Alignment = [System.Drawing.StringAlignment]::Center
        $fmt.LineAlignment = [System.Drawing.StringAlignment]::Center
        $rect = New-Object System.Drawing.RectangleF 0, 0, ($s * $scale), ($s * $scale)
        $g.DrawString('F', $font, $fg, $rect, $fmt)
        $font.Dispose()
        $fg.Dispose()

        $small = $bmp.GetThumbnailImage($s, $s, { $false }, [IntPtr]::Zero)
        $path = Join-Path $out $t.Name
        $small.Save($path, [System.Drawing.Imaging.ImageFormat]::Png)
        $small.Dispose()
        Write-Output ("asset " + $t.Name + " " + $s + "x" + $s + " " + (Get-Item $path).Length + " B")
    }
    finally {
        $g.Dispose()
        $bmp.Dispose()
    }
}
