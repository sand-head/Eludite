Option Strict On
Imports System

' Eludite corpus (MIT): a For Each whose loop variable declares its type.

Module Loops
    Sub Main(args As String())
        For Each arg As String In args
            Console.WriteLine(arg)
        Next
    End Sub
End Module
