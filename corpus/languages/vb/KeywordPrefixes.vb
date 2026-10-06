Option Strict On
Imports System

' Eludite corpus (MIT): identifiers that start with a keyword, where that keyword is valid.
' The grammar's keyword tokens take lexical precedence over the longer identifier, so Document, Format,
' Subtotal, Notify and Subscriptions lex as Do, For, Sub, Not and Sub.

Public Class Report
    Private Subscriptions As Integer

    Public Sub Run(document As Document, subtotal As Integer)
        Document.Save(document)
        Format(subtotal)
        Dim total = subtotal
        Dim sent = Notify()
    End Sub

    Private Function Notify() As Boolean
        Return True
    End Function
End Class
