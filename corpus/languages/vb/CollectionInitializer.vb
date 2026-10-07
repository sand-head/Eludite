Option Strict On
Imports System.Collections

' Eludite corpus (MIT): a collection initializer with From.

Module Initializers
    Sub Main()
        Dim list As ArrayList = New ArrayList From {1, 2, 3}
    End Sub
End Module
