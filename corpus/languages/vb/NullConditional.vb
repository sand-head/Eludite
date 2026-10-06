Option Strict On
Imports System

' Eludite corpus (MIT): the null-conditional member access ?.

Module NullConditional
    Sub Main(args As String())
        Dim first = args.FirstOrDefault()
        Console.WriteLine(first?.Length)
    End Sub
End Module
