Option Strict On
Imports System

' Eludite corpus (MIT): #Const, #Region and #If between declarations. The grammar allows a directive only inside a procedure.

#Const TRACE_LEVEL = 2

Public Class Settings

#Region "Fields"
    Private level As Integer = 1
#End Region

#If DEBUG Then
    Public ReadOnly Property Mode As String = "Debug"
#Else
    Public ReadOnly Property Mode As String = "Release"
#End If

#Region "Methods"
    Public Function Level() As Integer
        Return level
    End Function
#End Region

End Class
