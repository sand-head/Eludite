Option Strict On
Imports System.IO
Imports System.Text

' Eludite corpus (MIT): the As New declaration form, in a field, a Dim and a Using.

Module AsNew
    Private builder As New StringBuilder()

    Sub Main()
        Dim text As New StringBuilder()
        Using reader As New StreamReader("in.txt")
            text.Append(reader.ReadLine())
        End Using
    End Sub
End Module
