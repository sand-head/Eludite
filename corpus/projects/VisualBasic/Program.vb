' The Visual Basic fixture of corpus/projects (brief 0063, MIT). Keep the marked lines: the host's VisualBasicTests
' find them by their text.
Option Strict On

Imports System

''' <summary>Greets a person by name.</summary>
Public Class Greeter
    ''' <summary>The name greeted.</summary>
    Public Property Name As String

    Public Sub New(name As String)
        Me.Name = name
    End Sub

    ''' <summary>The greeting for <see cref="Name"/>.</summary>
    Public Function Greet() As String
        Return $"Hello, {Name}!"
    End Function
End Class

Module Program
    Sub Main()
        Dim greeter As New Greeter("World")
        Console.WriteLine(greeter.Greet())
        Dim n As Integer = "x" ' BC30512: Option Strict On disallows implicit conversions from 'String' to 'Integer'.
        Console.WriteLine(n)
    End Sub
End Module
