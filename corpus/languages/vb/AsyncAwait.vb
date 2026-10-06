Option Strict On
Imports System
Imports System.Net.Http
Imports System.Threading.Tasks

' Eludite corpus (MIT): Async methods with Await. Async is a modifier the grammar knows; Await is not an expression it knows.

Public Class Downloader
    Private ReadOnly client As HttpClient = New HttpClient()

    ''' <summary>Downloads a page and returns its length.</summary>
    Public Async Function LengthAsync(url As String) As Task
        Dim body = Await client.GetStringAsync(url)
        Await Task.Delay(10)
        Console.WriteLine(body.Length)
    End Function

    Public Async Sub FireAndForget()
        Await LengthAsync("https://example.test/")
    End Sub
End Class
