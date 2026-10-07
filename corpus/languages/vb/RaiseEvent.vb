Option Strict On
Imports System

' Eludite corpus (MIT): event declarations and the RaiseEvent statement.

Public Class Counter
    Private value As Integer

    ''' <summary>Raised after every increment.</summary>
    Public Event Incremented As EventHandler
    Public Event Changed(sender As Object, oldValue As Integer, newValue As Integer)

    Public Sub Increment()
        Dim old = value
        value += 1
        RaiseEvent Incremented(Me, EventArgs.Empty)
        RaiseEvent Changed(Me, old, value)
    End Sub
End Class
