Option Strict On

' Eludite corpus (MIT): array declarators on a Dim: Dim arr() As T = {...} and Dim arr(n) As T.

Module Arrays
    Sub Main()
        Dim numbers() As Integer = {1, 2, 3}
        Dim pieces(2) As String
        pieces(0) = numbers(0).ToString()
    End Sub
End Module
