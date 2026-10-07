Option Strict On
Imports System.Collections

' Eludite corpus (MIT): an Iterator function with Yield.

Public Class Pages
    Public Iterator Function All() As IEnumerable
        Yield "one"
        Yield "two"
    End Function
End Class
