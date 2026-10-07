Option Strict On
Imports System

' Eludite corpus (MIT): a Custom Event with AddHandler, RemoveHandler and RaiseEvent accessors.

Public Class Relay
    Private handlers As EventHandler

    Public Custom Event Forwarded As EventHandler
        AddHandler(value As EventHandler)
            handlers = CType([Delegate].Combine(handlers, value), EventHandler)
        End AddHandler
        RemoveHandler(value As EventHandler)
            handlers = CType([Delegate].Remove(handlers, value), EventHandler)
        End RemoveHandler
        RaiseEvent(sender As Object, e As EventArgs)
            handlers.Invoke(sender, e)
        End RaiseEvent
    End Event
End Class
