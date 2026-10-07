Option Strict On
Imports System
Imports System.Linq

' Eludite corpus (MIT): LINQ query expressions (From ... Where ... Order By ... Select).

Public Module Queries
    Public Function EvenSquares(numbers As Integer()) As Integer()
        Dim query = From n In numbers
                    Where n Mod 2 = 0
                    Order By n Descending
                    Select n * n
        Return query.ToArray()
    End Function
End Module
