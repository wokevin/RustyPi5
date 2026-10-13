$logPath = "$PSScriptRoot\rustypi5-com1.log"
Write-Host "Logging COM1 output to: $logPath"

$pipe = New-Object System.IO.Pipes.NamedPipeClientStream(".", "RustyPi5-HyperV-com1", [System.IO.Pipes.PipeDirection]::In)
$pipe.Connect(15000)
Write-Host "Connected."

$reader = New-Object System.IO.StreamReader($pipe)
$writer = New-Object System.IO.StreamWriter($logPath, $true)  # append mode
$writer.AutoFlush = $true

while ($true) {
	$ch = $reader.Read()
	if ($ch -ge 0) {
		$writer.Write([char]$ch)
		Write-Host -NoNewline ([char]$ch)
	}
}
