Option Strict On
Imports System

' Eludite corpus (MIT): operator declarations, including a conversion operator.

Public Structure Money
    Private ReadOnly cents As Long

    Public Sub New(cents As Long)
        Me.cents = cents
    End Sub

    Public Shared Operator +(left As Money, right As Money) As Money
        Return New Money(left.cents + right.cents)
    End Operator

    Public Shared Widening Operator CType(cents As Long) As Money
        Return New Money(cents)
    End Operator
End Structure
