Imports Microsoft.VisualStudio.TestTools.UnitTesting

' The RootNamespace is Corpus.VisualBasic, so the class is Corpus.VisualBasic.ParserTests.
<TestClass>
Public Class ParserTests

    Public Property TestContext As TestContext

    <TestMethod>
    Public Sub ParsesNumbers()
        Dim value = Integer.Parse("42")
        Assert.AreEqual(42, value)
    End Sub

    <TestMethod>
    Public Sub ParsesNegatives()
        Dim value = Integer.Parse("-7")
        Assert.AreEqual(7, value, "Negatives keep their sign")
    End Sub

    <TestMethod>
    <Ignore("Hexadecimal comes later")>
    Public Sub ParsesHex()
        Assert.Fail("not run")
    End Sub

    <TestMethod>
    Public Sub WritesOutput()
        TestContext.WriteLine("Hello from Visual Basic")
        Console.WriteLine("Console from Visual Basic")
    End Sub

    <TestMethod>
    Public Sub TrimsBeforeParsing()
        Dim value = Integer.Parse(" 12 ".Trim())
        Assert.AreEqual(12, value)
    End Sub

End Class
