Option Strict On
Imports System
Imports System.Collections.Generic

' Eludite corpus (MIT): generic types, generic declarations and constraints.
' The grammar's Of-lists have no parentheses, so no List(Of T), Class C(Of T) or Function F(Of T)() parses.

Namespace Eludite.Corpus.Generics

    Public Class Registry(Of T As {IComparable, Class, New})
        Private items As Dictionary(Of String, T)

        Public Function Largest(Of TResult As IComparable(Of TResult))(selector As Func(Of T, TResult)) As TResult
            Dim best As TResult = Nothing
            Return best
        End Function
    End Class

End Namespace
