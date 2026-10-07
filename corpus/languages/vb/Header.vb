' Eludite corpus (MIT): a comment line before Option, and a blank line between Option and Imports.
' Both are how every real file starts; the grammar accepts no comment or blank line before or between them.
Option Strict On

Imports System

Module Header
    Sub Main()
        Console.WriteLine("header")
    End Sub
End Module
