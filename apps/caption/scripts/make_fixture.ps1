Add-Type -AssemblyName System.Speech
$s = New-Object System.Speech.Synthesis.SpeechSynthesizer
$fmt = New-Object System.Speech.AudioFormat.SpeechAudioFormatInfo(16000, [System.Speech.AudioFormat.AudioBitsPerSample]::Sixteen, [System.Speech.AudioFormat.AudioChannel]::Mono)
New-Item -ItemType Directory -Force "$PSScriptRoot\..\tests\fixtures" | Out-Null
$s.SetOutputToWaveFile("$PSScriptRoot\..\tests\fixtures\en-10s.wav", $fmt)
$s.Speak("Good morning everyone. Today we will review the quarterly roadmap, discuss the migration to the new data center, and confirm the budget for GPU servers before the end of this month.")
$s.Dispose()
