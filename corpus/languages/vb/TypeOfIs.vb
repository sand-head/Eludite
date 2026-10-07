Option Strict On
Imports System

' Eludite corpus (MIT): the TypeOf ... Is type test.

Module TypeTests
    Sub Main(args As String())
        Dim value As Object = args
        Console.WriteLine(TypeOf value Is String())
    End Sub
End Module
