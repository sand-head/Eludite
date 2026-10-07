Option Strict On
Imports System
Imports System.Timers

' Eludite corpus (MIT): a WithEvents field and methods declared with Handles.

Public Class Clock
    Private WithEvents timer As Timer = New Timer(1000)

    Public Event Finished As EventHandler

    Public Sub Start()
        timer.Start()
    End Sub

    Private Sub OnTick(sender As Object, e As ElapsedEventArgs) Handles timer.Elapsed
        Console.WriteLine(e.SignalTime)
    End Sub

    Private Sub OnDisposed(sender As Object, e As EventArgs) Handles timer.Disposed, Me.Finished
        Console.WriteLine("stopped")
    End Sub
End Class
