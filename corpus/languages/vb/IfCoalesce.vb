Option Strict On
Imports System

' Eludite corpus (MIT): the two-argument If operator (null coalescing).

Module Coalesce
    Sub Main(args As String())
        Dim first As String = If(args.Length > 0, args(0), Nothing)
        Console.WriteLine(If(first, "fallback"))
    End Sub
End Module
