Option Strict On
Imports System.Text

' Eludite corpus (MIT): a With block whose statements start with the implicit member access `.Member`.

Module Builders
    Sub Main()
        Dim builder = New StringBuilder()
        With builder
            .Append("Hello")
            .Append(", ")
            .AppendLine("world")
        End With
    End Sub
End Module
