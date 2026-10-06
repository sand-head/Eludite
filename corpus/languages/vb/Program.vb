Option Strict On
Option Explicit On
Option Infer On
Imports System
Imports System.IO
Imports System.Text

' Eludite corpus: a console program in Visual Basic (MIT).
' Statements, control flow, literals and lambdas that tree-sitter-vb-dotnet parses clean.
' The Option and Imports lines come first with nothing between them: see Header.vb.

Namespace Eludite.Corpus

    ''' <summary>
    ''' The entry point and a few procedures showing control flow.
    ''' </summary>
    Public Module Program

        ''' <summary>A count of the runs so far.</summary>
        Private runs As Integer = 0

        Private Const Greeting As String = "Hello"
        Private Const MaxRetries% = 3
        Private ReadOnly Started As Date = #1/15/2026 09:30:00 AM#

        ''' <summary>Runs the program.</summary>
        ''' <param name="args">The command line.</param>
        Sub Main(args As String())
            Dim name = If(args.Length > 0, args(0), "world")
            Dim total As Long = 0L
            Dim ratio As Double = 1.5R
            Dim price@ = 19.99@
            Dim flag! = 2.5!
            Dim mask As Integer = &H7F Or &O17
            Dim letter As Char = "A"c
            Dim quoted As String = "She said ""hi"""
            Dim message = $"{Greeting}, {name}! Run {runs + 1} of {MaxRetries}: {ratio:F2}"
            Console.WriteLine(message)
            runs += 1

            If runs > MaxRetries Then
                Console.WriteLine("Too many runs")
            ElseIf runs = MaxRetries Then
                Console.WriteLine("Last run")
            Else
                Console.WriteLine("Running")
            End If

            For i As Integer = 1 To 10 Step 2
                total += i
            Next

            For j = 10 To 1 Step -1
                If j Mod 2 = 0 Then Continue For
                total -= j
            Next j

            Dim words = name.Split(","c)
            Dim numbers = {1, 2, 3}
            ReDim Preserve numbers(5)
            For Each word In words
                Console.WriteLine(word.ToUpper())
            Next

            Dim k = 0
            Do While k < 3
                k += 1
            Loop

            Do
                k -= 1
            Loop Until k = 0

            While total > 0
                total \= 2
                If total < 10 Then Exit While
            End While

            Select Case name.Length
                Case 0
                    Console.WriteLine("empty")
                Case 1 To 3, 5
                    Console.WriteLine("short")
                Case Is > 20
                    Console.WriteLine("long")
                Case Else
                    Console.WriteLine("medium")
            End Select

            Dim square = Function(x As Integer) x * x
            Dim log = Sub(text As String)
                          Console.Error.WriteLine(text)
                      End Sub
            Dim later = Function()
                            Return Started.AddDays(1)
                        End Function
            log(square(7).ToString())
            Console.WriteLine(later().ToString("yyyy-MM-dd"))

            Dim builder = New StringBuilder()
            builder.Append(Greeting).Append(", ").AppendLine(name)
            With builder
                Console.WriteLine(builder.Length)
            End With

            Try
                Dim text = ReadFirstLine("missing.txt")
                Console.WriteLine(text)
            Catch ex As FileNotFoundException When ex.FileName IsNot Nothing
                Console.WriteLine($"Not found: {ex.FileName}")
            Catch ex As IOException
                Console.WriteLine(ex.Message)
            Catch
                Throw
            Finally
                Console.WriteLine("done")
            End Try

            Dim result = Describe(total, _
                                  name, _
                                  Nothing)
            Console.WriteLine(result)
            Console.WriteLine(result Like "H*")
            GoTo Finish
Finish:
            Console.WriteLine(Not False AndAlso True OrElse False Xor True)
        End Sub

        ''' <summary>Reads the first line of a file.</summary>
        Private Function ReadFirstLine(path As String) As String
            Using reader = New StreamReader(path)
                Return reader.ReadLine()
            End Using
        End Function

        Private Function Describe(total As Long, name As String, Optional suffix As String = Nothing, ParamArray rest() As Object) As String
            Dim pieces = New String() {"", "", ""}
            SyncLock pieces
                pieces(0) = total.ToString()
                pieces(1) = name
                pieces(2) = CStr(If(suffix Is Nothing, String.Empty, suffix))
            End SyncLock
            Return String.Join(" ", pieces) & " " & rest.Length.ToString()
        End Function

        Private Sub Parse(ByVal text As String, ByRef value As Integer)
            value = Integer.Parse(text)
#If DEBUG Then
            Console.WriteLine("parsed")
#End If
        End Sub

    End Module

End Namespace
