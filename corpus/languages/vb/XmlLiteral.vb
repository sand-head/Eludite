Option Strict On
Imports System
Imports System.Xml.Linq

' Eludite corpus (MIT): an XML literal with embedded expressions.

Public Module Documents
    Public Function Describe(name As String, count As Integer) As XElement
        Dim document = <item name=<%= name %>>
                           <count><%= count %></count>
                       </item>
        Return document
    End Function
End Module
