# BioGit Excel-мост: JSON-протокол поверх stdin/stdout с COM-автоматизацией Excel.
#
# Протокол: на каждую строку stdin {"id":N,"op":"...","args":{...}} мост отвечает
# одной строкой stdout {"id":N,"ok":true,"data":{...}} или
# {"id":N,"ok":false,"error":"..."}.
#
# Excel запускается невидимым, диалоги подавлены, макросы отключены.
# Работает с любым установленный Office (включая Microsoft 365 / Click-to-Run):
# Windows сама находит Excel.Application через реестр.

$ErrorActionPreference = 'Stop'
try { [Console]::OutputEncoding = New-Object System.Text.UTF8Encoding($false) } catch {}
try { [Console]::InputEncoding  = New-Object System.Text.UTF8Encoding($false) } catch {}

$script:excel = $null
$script:books = @{}

function Respond([object]$obj) {
    [Console]::Out.WriteLine(($obj | ConvertTo-Json -Compress -Depth 8))
}

function Fail([int]$id, [string]$msg) {
    Respond @{ id = $id; ok = $false; error = $msg }
}

function Ensure-Excel {
    if ($null -eq $script:excel) {
        $script:excel = New-Object -ComObject Excel.Application
        $script:excel.Visible = $false
        $script:excel.DisplayAlerts = $false
        try { $script:excel.AutomationSecurity = 3 } catch {} # макросы отключены
    }
}

function Get-Book([string]$path) {
    if ($script:books.ContainsKey($path)) { return $script:books[$path] }
    Ensure-Excel
    $wb = $script:excel.Workbooks.Open($path, 0, $false)
    $script:books[$path] = $wb
    return $wb
}

function Normalize($v) {
    if ($null -eq $v) { return $null }
    if ($v -is [System.DBNull]) { return $null }
    if ($v -is [double]) { return $v }
    if ($v -is [int] -or $v -is [long]) { return [double]$v }
    if ($v -is [bool]) { return $v }
    return [string]$v
}

:mainloop while ($true) {
    $line = [Console]::In.ReadLine()
    if ($null -eq $line) { break }
    if ([string]::IsNullOrWhiteSpace($line)) { continue }

    $req = $null
    try { $req = $line | ConvertFrom-Json } catch { Fail 0 ("не удалось разобрать запрос: " + $_.Exception.Message); continue }
    $id = 0
    try { $id = [int]$req.id } catch { $id = 0 }
    $op = [string]$req.op

    try {
        switch ($op) {
            'ping' {
                Ensure-Excel
                Respond @{ id = $id; ok = $true; data = @{ version = [string]$script:excel.Version } }
            }
            'open' {
                $p = [string]$req.args.path
                $wb = Get-Book $p
                $names = @()
                foreach ($s in $wb.Worksheets) { $names += [string]$s.Name }
                Respond @{ id = $id; ok = $true; data = @{ sheets = $names } }
            }
            'read' {
                $p = [string]$req.args.path
                $sheet = [string]$req.args.sheet
                $wb = Get-Book $p
                $ws = $wb.Worksheets.Item($sheet)
                $ur = $ws.UsedRange
                $r0 = [int]$ur.Row
                $c0 = [int]$ur.Column
                $nr = [int]$ur.Rows.Count
                $nc = [int]$ur.Columns.Count
                $total = $nr * $nc
                $values = New-Object 'object[]' $total
                $formulas = New-Object 'object[]' $total
                if ($nr -eq 1 -and $nc -eq 1) {
                    $values[0] = Normalize $ur.Value2
                    $f = [string]$ur.Formula
                    if ($f -and $f -ne '') { $formulas[0] = $f } else { $formulas[0] = $null }
                } else {
                    $vals = $ur.Value2
                    $fs = $ur.Formula
                    for ($i = 1; $i -le $nr; $i++) {
                        for ($j = 1; $j -le $nc; $j++) {
                            $idx = ($i - 1) * $nc + ($j - 1)
                            $values[$idx] = Normalize $vals[$i, $j]
                            $f = $fs[$i, $j]
                            if ($f -is [string] -and $f -ne '') { $formulas[$idx] = $f } else { $formulas[$idx] = $null }
                        }
                    }
                }
                Respond @{ id = $id; ok = $true; data = @{ row = $r0; col = $c0; rows = $nr; cols = $nc; values = $values; formulas = $formulas } }
            }
            'edit' {
                $p = [string]$req.args.path
                $sheet = [string]$req.args.sheet
                $row = [int]$req.args.row
                $col = [int]$req.args.col
                $val = [string]$req.args.input
                $wb = Get-Book $p
                $ws = $wb.Worksheets.Item($sheet)
                $cell = $ws.Cells.Item($row, $col)
                if ($val.StartsWith('=')) {
                    $cell.Formula = $val
                } elseif ($val -eq '') {
                    $cell.ClearContents() | Out-Null
                } elseif ($val -match '^-?[0-9]+(\.[0-9]+)?$') {
                    $cell.Value2 = [double]::Parse($val, [System.Globalization.CultureInfo]::InvariantCulture)
                } elseif ($val -ieq 'TRUE' -or $val -ieq 'FALSE') {
                    $cell.Value2 = ($val -ieq 'TRUE')
                } else {
                    $cell.Value2 = $val
                }
                $v = Normalize $cell.Value2
                $f = [string]$cell.Formula
                if ($f -and $f -ne '') {
                    Respond @{ id = $id; ok = $true; data = @{ value = $v; formula = $f } }
                } else {
                    Respond @{ id = $id; ok = $true; data = @{ value = $v; formula = $null } }
                }
            }
            'save' {
                $p = [string]$req.args.path
                $wb = $script:books[$p]
                if ($null -eq $wb) { throw "рабочая книга не открыта: $p" }
                $wb.Save()
                Respond @{ id = $id; ok = $true; data = @{} }
            }
            'close' {
                $p = [string]$req.args.path
                $wb = $script:books[$p]
                if ($null -ne $wb) {
                    $wb.Close($false) | Out-Null
                    $script:books.Remove($p) | Out-Null
                }
                Respond @{ id = $id; ok = $true; data = @{} }
            }
            'quit' {
                foreach ($wb in $script:books.Values) {
                    try { $wb.Close($false) | Out-Null } catch {}
                }
                $script:books.Clear()
                if ($null -ne $script:excel) {
                    try { $script:excel.Quit() } catch {}
                    try { [System.Runtime.InteropServices.Marshal]::ReleaseComObject($script:excel) | Out-Null } catch {}
                    $script:excel = $null
                }
                Respond @{ id = $id; ok = $true; data = @{} }
                break mainloop
            }
            default {
                Fail $id ("неизвестная операция: " + $op)
            }
        }
    } catch {
        Fail $id $_.Exception.Message
    }
}
