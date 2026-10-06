Option Strict On
Imports System

' Eludite corpus (MIT): the AddHandler and RemoveHandler statements.

Public Class Watcher
    Private ReadOnly counter As Counter

    Public Sub New(counter As Counter)
        Me.counter = counter
        AddHandler counter.Incremented, AddressOf OnIncremented
        AddHandler counter.Changed, Sub(sender, oldValue, newValue) Console.WriteLine(newValue)
    End Sub

    Public Sub Detach()
        RemoveHandler counter.Incremented, AddressOf OnIncremented
    End Sub

    Private Sub OnIncremented(sender As Object, e As EventArgs)
        Console.WriteLine("incremented")
    End Sub
End Class
