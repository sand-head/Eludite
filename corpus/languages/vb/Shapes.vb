Option Strict On
Imports System
Imports System.ComponentModel

' Eludite corpus: types in Visual Basic (MIT).
' Classes, structures, interfaces, enums, delegates, properties and attributes that the grammar parses clean.
' Inherits and Implements share the Class line (see InheritsLine.vb); nothing is generic (see Generics.vb);
' no For Each variable has a type (see ForEachTyped.vb).

Namespace Eludite.Corpus.Shapes

    ''' <summary>Anything with an area.</summary>
    Public Interface IShape
        ReadOnly Property Name As String
        Function Area() As Double
        Sub Scale(factor As Double)
    End Interface

    Public Enum Color
        Red
        Green = 5
        Blue
    End Enum

    <Flags>
    Public Enum Edges
        None = 0
        Top = 1
        Bottom = 2
        All = Top Or Bottom
    End Enum

    Public Delegate Function Measure(shape As IShape) As Double
    Public Delegate Sub Report(message As String)

    ''' <summary>A point with value semantics.</summary>
    <Serializable>
    Public Structure Point
        Public X As Double
        Public Y As Double

        Public Sub New(x As Double, y As Double)
            Me.X = x
            Me.Y = y
        End Sub

        Public Overrides Function ToString() As String
            Return $"({X}, {Y})"
        End Function
    End Structure

    ''' <summary>The base of every shape.</summary>
    Public MustInherit Class Shape Implements IShape

        Private Shared count As Integer
        Protected ReadOnly center As Point

        Public Shared ReadOnly Property Count As Integer
            Get
                Return count
            End Get
        End Property

        Public Property Fill As Color = Color.Red
        Public Property Tag As Object

        Protected Sub New(center As Point)
            Me.center = center
            count += 1
        End Sub

        Public MustOverride ReadOnly Property Name As String
        Public MustOverride Function Area() As Double

        Public Overridable Sub Scale(factor As Double)
        End Sub

        Public Overloads Function Describe() As String
            Return Describe(Fill)
        End Function

        Public Overloads Function Describe(color As Color) As String
            Return $"{Name} in {color} at {center}"
        End Function
    End Class

    ''' <summary>A circle.</summary>
    Public NotInheritable Class Circle Inherits Shape

        Private radius As Double

        Public Sub New(center As Point, radius As Double)
            MyBase.New(center)
            Me.radius = radius
        End Sub

        <Description("The circle's radius")>
        Public Property Radius As Double
            Get
                Return radius
            End Get
            Set(value As Double)
                If value < 0 Then Throw New ArgumentOutOfRangeException(NameOf(value))
                radius = value
            End Set
        End Property

        Public Overrides ReadOnly Property Name As String
            Get
                Return "Circle"
            End Get
        End Property

        Public Overrides Function Area() As Double
            Return Math.PI * radius ^ 2
        End Function

        Public Overrides Sub Scale(factor As Double)
            radius *= factor
        End Sub
    End Class

    Public Class Canvas
        Private shapes As ArrayList = New ArrayList()
        Private origin As Point = New Point(0, 0)
        Private settings As Options = New Options With {.Width = 800, .Height = 600}

        Default Public ReadOnly Property Item(index As Integer) As IShape
            Get
                Return CType(shapes(index), IShape)
            End Get
        End Property

        Public Sub Add(shape As IShape)
            shapes.Add(shape)
        End Sub

        Public Function Total(measure As Measure) As Double
            Dim sum As Double = 0
            For Each shape In shapes
                sum += measure(CType(shape, IShape))
            Next
            Return sum
        End Function
    End Class

    Public Class Options
        Public Property Width As Integer
        Public Property Height As Integer
    End Class

End Namespace
