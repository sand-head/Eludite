Option Strict On
Imports System

' Eludite corpus (MIT): the Implements clause on a property and a method.

Public Interface IMoney
    ReadOnly Property Amount As Decimal
    Function Add(other As IMoney) As IMoney
End Interface

Public Class Money Implements IMoney
    Private ReadOnly cents As Long

    Public ReadOnly Property Amount As Decimal Implements IMoney.Amount
        Get
            Return cents / 100D
        End Get
    End Property

    Public Function Add(other As IMoney) As IMoney Implements IMoney.Add
        Return New Money()
    End Function
End Class
