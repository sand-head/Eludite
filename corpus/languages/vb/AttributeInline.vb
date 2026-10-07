Option Strict On
Imports System

' Eludite corpus (MIT): an attribute on the same line as the declaration it decorates.

<Serializable> Public Class Settings
    <Obsolete("use Name")> Public Property Title As String
End Class
