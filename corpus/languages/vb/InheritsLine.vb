Option Strict On
Imports System

' Eludite corpus (MIT): Inherits and Implements on their own lines after the Class line, as Visual Studio writes them.

Namespace Eludite.Corpus.Inheritance

    Public Interface INamed
        ReadOnly Property Name As String
    End Interface

    Public Class Base
    End Class

    Public Class Derived
        Inherits Base
        Implements INamed

        Public ReadOnly Property Name As String
            Get
                Return "derived"
            End Get
        End Property
    End Class

End Namespace
